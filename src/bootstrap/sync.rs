use super::{
    auth::{bind_account, open_authorized_telegram},
    database::{db_error, open_existing_store},
    print_output,
    runtime::start_sync_runtime,
};
use crate::{
    application::{SyncJob, SyncJobState, SyncScope},
    config::{Config, SyncPacing},
    infrastructure::{persistence::sqlite::SqliteStore, telegram::TelegramAdapter},
    interface::cli::{CliError, OutputFormat, render_sync_job},
};
use std::sync::Arc;

async fn sync_scope_allowed(database_url: &str, scope: &SyncScope) -> Result<bool, CliError> {
    use crate::application::ChatRepository;
    let store = open_existing_store(database_url).await?;
    let result: Result<bool, CliError> = async {
        match scope {
            SyncScope::Chat(id) => match store.get(*id).await.map_err(db_error)? {
                None => Err(CliError::ChatNotFound(id.get())),
                Some(chat) if !chat.tracked => {
                    Err(crate::application::ApplicationError::NotTracked.into())
                }
                Some(_) => Ok(true),
            },
            SyncScope::All => Ok(store
                .list()
                .await
                .map_err(db_error)?
                .iter()
                .any(|chat| chat.tracked)),
        }
    }
    .await;
    store.close().await;
    result
}

/// `chats track --backfill` (no server): the chat is already tracked and committed; run the same
/// in-process history sync as `sync chat`. Missing Telegram credentials leave tracking in place
/// and fail with instructions for running the sync later.
pub(super) async fn backfill_after_track(
    chat_id: crate::domain::ChatId,
    output: OutputFormat,
) -> Result<(), CliError> {
    if let Err(reason) = Config::telegram() {
        return Err(CliError::InvalidInput(format!(
            "chat {} is tracked, but the backfill was not run: {reason}. Set the Telegram credentials and run `tgarchive sync chat {}` (or track via the running server with PUT /api/v1/chats/{}/tracking?backfill=true)",
            chat_id.get(),
            chat_id.get(),
            chat_id.get()
        )));
    }
    sync_archive(SyncScope::Chat(chat_id), output, false).await
}

pub(super) async fn sync_archive(
    scope: SyncScope,
    output: OutputFormat,
    refetch: bool,
) -> Result<(), CliError> {
    SyncPacing::from_env().map_err(CliError::InvalidInput)?;
    let config = Config::load(None);
    if !sync_scope_allowed(&config.database_url, &scope).await? {
        print_output(if output == OutputFormat::Json {
            "{\"tracked_chats\":0,\"synced\":false}".into()
        } else {
            "No tracked chats; nothing to sync. Track chats with `chats track <CHAT_ID>`.".into()
        });
        return Ok(());
    }
    let telegram = Config::telegram().map_err(CliError::InvalidInput)?;
    SyncPacing::from_env().map_err(CliError::InvalidInput)?;
    let adapter = open_authorized_telegram(&telegram).await?;
    let result = sync_with_adapter(&config.database_url, adapter.clone(), scope, refetch).await;
    let shutdown = adapter
        .shutdown()
        .await
        .map_err(|_| CliError::Telegram("Telegram connection did not shut down cleanly".into()));
    let job = result?;
    shutdown?;
    if job.state != SyncJobState::Succeeded {
        return Err(CliError::SyncFailed(
            job.summary_error
                .clone()
                .unwrap_or_else(|| format!("job ended as {:?}", job.state)),
        ));
    }
    print_output(render_sync_job(output, &job)?);
    Ok(())
}

async fn sync_with_adapter(
    database_url: &str,
    adapter: Arc<TelegramAdapter>,
    scope: SyncScope,
    refetch: bool,
) -> Result<SyncJob, CliError> {
    let store = Arc::new(
        SqliteStore::connect(database_url)
            .await
            .map_err(|error| CliError::Database(error.to_string()))?,
    );
    let setup = async {
        bind_account(&store, &adapter).await?;
        Ok::<(), CliError>(())
    }
    .await;
    if let Err(error) = setup {
        store.close().await;
        return Err(error);
    }
    let runtime = match start_sync_runtime(store.clone(), adapter).await {
        Ok(runtime) => runtime,
        Err(error) => {
            store.close().await;
            return Err(error);
        }
    };
    let result = async {
        let job = runtime.coordinator.submit_with(scope, refetch).await?;
        runtime
            .coordinator
            .wait_job(&job.id)
            .await
            .map_err(CliError::from)
    }
    .await;
    let stopped = runtime.shutdown().await;
    store.close().await;
    let job = result?;
    stopped?;
    Ok(job)
}

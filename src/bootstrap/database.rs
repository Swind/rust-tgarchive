use super::print_output;
use crate::{
    application::services::{Application, ComponentState, ComponentStatus},
    config::Config,
    infrastructure::persistence::sqlite::{RebuildProgress, SqliteStore},
    interface::cli::{CliError, OutputFormat},
};
use std::sync::Arc;

const REBUILD_BATCH: u32 = 1000;

pub(super) async fn rebuild_index(store: &SqliteStore) -> Result<RebuildProgress, CliError> {
    let mut last_reported = 0;
    store
        .rebuild_search_index(REBUILD_BATCH, |progress| {
            if progress.indexed == 0
                || progress.indexed - last_reported >= 10_000
                || progress.indexed == progress.total
            {
                last_reported = progress.indexed;
                eprintln!(
                    "rebuilding search index: {}/{} messages",
                    progress.indexed, progress.total
                );
            }
        })
        .await
        .map_err(db_error)
}

/// `db init` brings a new or upgraded archive's search index up to date (progress on stderr).
pub(super) async fn rebuild_index_if_needed(store: &SqliteStore) -> Result<(), CliError> {
    if store.search_index_ready().await.map_err(db_error)? {
        return Ok(());
    }
    eprintln!(
        "search index is missing or outdated; rebuilding it (Chinese-friendly full-text search)"
    );
    rebuild_index(store).await?;
    Ok(())
}

/// `serve` never blocks on a rebuild: it starts, reports the index state in `/api/v1/status` and
/// searches with the slower LIKE fallback until `tgarchive search rebuild-index` has run.
pub(super) async fn warn_if_index_not_ready(store: &SqliteStore) {
    if matches!(store.search_index_ready().await, Ok(false)) {
        tracing::warn!(
            "search index is not ready; searches use a slower substring scan. Run `tgarchive search rebuild-index` (or `db init`)"
        );
    }
}

pub(super) async fn readonly_application(
    database_url: &str,
    collector_detail: &str,
) -> Result<Arc<Application>, CliError> {
    let store = Arc::new(
        SqliteStore::open_existing_readonly(database_url)
            .await
            .map_err(|error| {
                CliError::Database(format!(
                    "cannot open archive database ({error}); create it with `tgarchive db init`"
                ))
            })?,
    );
    warn_if_index_not_ready(&store).await;
    Ok(Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store,
        None,
        ComponentStatus::new(ComponentState::Disabled, Some(collector_detail.to_owned())),
    )))
}

pub(super) async fn open_existing_store(database_url: &str) -> Result<Arc<SqliteStore>, CliError> {
    SqliteStore::open_existing(database_url)
        .await
        .map(Arc::new)
        .map_err(|error| {
            CliError::Database(format!(
                "cannot open archive database ({error}); create it with `tgarchive db init`"
            ))
        })
}

/// Checks the tracking scope against the local database before Telegram is contacted.
/// Returns `false` when there is nothing to sync (`sync all` with no tracked chats).
pub(super) fn db_error(error: crate::application::RepositoryError) -> CliError {
    CliError::Database(error.to_string())
}

/// `repair senders`: local-only; `--dry-run` opens the database read-only.
pub(super) async fn repair_senders(dry_run: bool, output: OutputFormat) -> Result<(), CliError> {
    let config = Config::load(None);
    let store = if dry_run {
        SqliteStore::open_existing_readonly(&config.database_url)
            .await
            .map(Arc::new)
            .map_err(db_error)?
    } else {
        open_existing_store(&config.database_url).await?
    };
    let report = store.repair_senders(dry_run, 1000).await;
    store.close().await;
    print_output(crate::interface::cli::render_sender_repair(
        output,
        &report.map_err(db_error)?,
    )?);
    Ok(())
}

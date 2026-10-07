use crate::{
    application::{
        ArchiveWriter, PageSize, RepositoryError, SyncRepository,
        ingestion_worker::{self, IngestSink},
        pacer::RatePacer,
        realtime::{ReconnectBackoff, supervise_realtime_with_status},
        services::CollectorStatusHandle,
        sync::{CancellationToken, SyncCoordinator, SyncEngine},
    },
    config::SyncPacing,
    infrastructure::{
        persistence::sqlite::SqliteStore,
        telegram::{TelegramAdapter, realtime::AdapterRealtime},
    },
    interface::cli::CliError,
};
use std::sync::Arc;
use tokio::task::JoinHandle;

pub(super) struct SyncRuntime {
    pub(super) coordinator: Arc<SyncCoordinator>,
    pub(super) engine: Arc<SyncEngine>,
    sink: IngestSink,
    pub(super) writer: JoinHandle<Result<(), RepositoryError>>,
    pub(super) realtime: Option<RealtimeTask>,
    pub(super) media: Option<MediaTask>,
}

pub(super) struct MediaTask {
    cancel: CancellationToken,
    pub(super) handle: JoinHandle<()>,
}

pub(super) struct RealtimeTask {
    cancel: CancellationToken,
    pub(super) handle: JoinHandle<Result<(), crate::application::ApplicationError>>,
}

impl SyncRuntime {
    pub(super) fn start_media(
        &mut self,
        adapter: Arc<TelegramAdapter>,
        store: Arc<SqliteStore>,
        directory: std::path::PathBuf,
    ) {
        let cancel = CancellationToken::new();
        let handle = crate::infrastructure::media::MediaWorker::spawn(
            adapter,
            (*store).clone(),
            directory,
            Arc::clone(self.engine.pacer()),
            cancel.clone(),
        );
        self.media = Some(MediaTask { cancel, handle });
    }
    /// Starts catch-up plus live updates on the same writer and engine as history sync.
    pub(super) fn start_realtime(
        &mut self,
        adapter: Arc<TelegramAdapter>,
        scope: Arc<dyn crate::application::TrackingScope>,
        status: CollectorStatusHandle,
    ) {
        let cancel = CancellationToken::new();
        let engine = Arc::clone(&self.engine);
        let source = AdapterRealtime::new(adapter, self.sink.clone(), scope);
        let token = cancel.clone();
        let handle = tokio::spawn(async move {
            supervise_realtime_with_status(
                &source,
                &engine,
                ReconnectBackoff::default(),
                token,
                &status,
            )
            .await
        });
        self.realtime = Some(RealtimeTask { cancel, handle });
    }

    pub(super) async fn shutdown(self) -> Result<(), CliError> {
        let Self {
            coordinator,
            engine,
            sink,
            writer,
            realtime,
            media,
        } = self;
        let media_result = match media {
            Some(MediaTask { cancel, mut handle }) => {
                cancel.cancel();
                match tokio::time::timeout(std::time::Duration::from_secs(10), &mut handle).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => {
                        Err(CliError::Telegram(format!("media worker failed: {error}")))
                    }
                    Err(_) => {
                        handle.abort();
                        let _ = handle.await;
                        Err(CliError::Telegram(
                            "media worker shutdown exceeded its deadline".into(),
                        ))
                    }
                }
            }
            None => Ok(()),
        };
        // Producers stop first so the writer can drain once every sender is dropped.
        let realtime_result = match realtime {
            Some(RealtimeTask { cancel, mut handle }) => {
                cancel.cancel();
                match tokio::time::timeout(std::time::Duration::from_secs(10), &mut handle).await {
                    Ok(joined) => joined
                        .map_err(|error| {
                            CliError::Telegram(format!("realtime task failed: {error}"))
                        })?
                        .map_err(|error| {
                            CliError::Telegram(format!("realtime updates failed: {error}"))
                        }),
                    Err(_) => {
                        handle.abort();
                        let _ = handle.await;
                        Err(CliError::Telegram(
                            "realtime shutdown exceeded its deadline; unacknowledged updates are re-fetched on restart".into(),
                        ))
                    }
                }
            }
            None => Ok(()),
        };
        if let Err(error) = &realtime_result {
            tracing::error!(%error, "realtime listener ended with an error");
        }
        let coordinator_result = coordinator
            .shutdown_with_timeout(std::time::Duration::from_secs(10))
            .await
            .map_err(|error| CliError::Telegram(error.to_string()));
        drop(coordinator);
        drop(engine);
        drop(sink);
        let mut writer = writer;
        let writer_result = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            &mut writer,
        )
        .await
        {
            Ok(result) => result
                .map_err(|error| {
                    CliError::Telegram(format!("ingestion writer task failed: {error}"))
                })?
                .map_err(|error| CliError::Telegram(format!("ingestion writer failed: {error}"))),
            Err(_) => {
                writer.abort();
                let _ = writer.await;
                Err(CliError::Telegram(
                    "ingestion writer shutdown exceeded its deadline; the database may require recovery on restart".into(),
                ))
            }
        };
        realtime_result?;
        media_result?;
        coordinator_result?;
        writer_result
    }
}

pub(super) async fn start_sync_runtime(
    store: Arc<SqliteStore>,
    adapter: Arc<TelegramAdapter>,
) -> Result<SyncRuntime, CliError> {
    let pacing = SyncPacing::from_env().map_err(CliError::InvalidInput)?;
    crate::application::SyncRepository::recover_interrupted(store.as_ref())
        .await
        .map_err(|error| CliError::Database(error.to_string()))?;
    let writer: Arc<dyn ArchiveWriter> = store.clone();
    let repository: Arc<dyn SyncRepository> = store.clone();
    let chats: Arc<dyn crate::application::ChatRepository> = store;
    let gateway: Arc<dyn crate::application::TelegramGateway> = adapter;
    let (sink, writer_task) = ingestion_worker::spawn(writer, 32);
    let engine = Arc::new(
        SyncEngine::new(
            gateway,
            repository.clone(),
            chats,
            sink.clone(),
            PageSize::DEFAULT,
        )
        .with_rate_control(
            Arc::new(RatePacer::new(pacing.page_delay)),
            pacing.max_flood_wait,
        ),
    );
    let coordinator = SyncCoordinator::spawn(engine.clone(), repository, 16);
    Ok(SyncRuntime {
        coordinator,
        engine,
        sink,
        writer: writer_task,
        realtime: None,
        media: None,
    })
}

use super::*;
use chrono::Utc;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, mpsc, watch};

enum Command {
    /// The flag selects a `--refetch` walk (chat scope only).
    Run(SyncJob, CancellationToken, bool),
}
struct Reservation {
    accepting: bool,
    all: bool,
    chats: HashSet<ChatId>,
}

/// A single consumer guarantees at most one history RPC stream at a time.
pub struct SyncCoordinator {
    sender: mpsc::Sender<Command>,
    repository: Arc<dyn SyncRepository>,
    chats: Arc<dyn ChatRepository>,
    reserved: Arc<Mutex<Reservation>>,
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    shutdown: watch::Sender<bool>,
    fatal: watch::Receiver<Option<String>>,
    worker: Mutex<Option<tokio::task::JoinHandle<Result<(), String>>>>,
    next_id: AtomicU64,
}

impl SyncCoordinator {
    pub fn spawn(
        engine: Arc<SyncEngine>,
        repository: Arc<dyn SyncRepository>,
        queue_capacity: usize,
    ) -> Arc<Self> {
        let (sender, mut receiver) = mpsc::channel::<Command>(queue_capacity.max(1));
        let reserved = Arc::new(Mutex::new(Reservation {
            accepting: true,
            all: false,
            chats: HashSet::new(),
        }));
        let cancellations = Arc::new(Mutex::new(HashMap::new()));
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let (fatal_tx, fatal) = watch::channel(None);
        let coordinator = Arc::new(Self {
            sender,
            repository: repository.clone(),
            chats: engine.chats.clone(),
            reserved: reserved.clone(),
            cancellations: cancellations.clone(),
            shutdown,
            fatal,
            worker: Mutex::new(None),
            next_id: AtomicU64::new(1),
        });
        let worker = tokio::spawn(async move {
            let mut result: Result<(), String> = async {
                loop {
                    let command = tokio::select! {
                        biased;
                        changed = shutdown_rx.changed() => {
                            if changed.is_ok() && *shutdown_rx.borrow() { receiver.close(); break; }
                            continue;
                        }
                        command = receiver.recv() => command,
                    };
                    let Some(Command::Run(mut job, cancel, refetch)) = command else {
                        break;
                    };
                    job.state = SyncJobState::Running;
                    job.started_at = Some(Utc::now());
                    if let Err(error) = repository.save_job(job.clone()).await {
                        job.state = SyncJobState::Interrupted;
                        job.completed_at = Some(Utc::now());
                        job.summary_error = Some(format!("cannot mark sync job running: {error}"));
                        let recovery = repository.save_job(job.clone()).await.err().map(|e| e.to_string());
                        release_reservation(&reserved, &job.scope).await;
                        cancellations.lock().await.remove(&job.id);
                        return Err(recovery.map_or_else(|| format!("cannot mark sync job running: {error}"), |e| format!("cannot mark sync job running: {error}; cannot mark it interrupted: {e}")));
                    }
                    let result = match job.scope.clone() {
                        SyncScope::Chat(id) => match if refetch {
                            engine.refetch_chat(&job.id, id, &cancel).await
                        } else {
                            engine.sync_chat(&job.id, id, &cancel).await
                        } {
                            Ok(_) => Ok(()),
                            Err(error) => {
                                let committed = repository
                                    .list_chat_progress(&job.id)
                                    .await
                                    .map_err(|e| format!("cannot read sync progress: {e}"))?
                                    .into_iter()
                                    .find(|item| item.chat_id == id)
                                    .map_or(0, |item| item.committed_messages);
                                engine
                                    .sink
                                    .submit(IngestBatch {
                                        job_progress: Some(SyncChatProgress {
                                            job_id: job.id.clone(),
                                            chat_id: id,
                                            state: failure_state(&error, &cancel),
                                            committed_messages: committed,
                                            summary_error: Some(failure_summary(&error)),
                                        }),
                                        ..Default::default()
                                    })
                                    .await
                                    .map_err(|e| {
                                        format!("cannot persist failed chat progress: {e}")
                                    })?;
                                Err(error)
                            }
                        },
                        SyncScope::All => run_all(&engine, &job.id, &cancel).await,
                    };
                    job.completed_at = Some(Utc::now());
                    job.state = match &result {
                        _ if cancel.is_cancelled() => SyncJobState::Interrupted,
                        Ok(()) => SyncJobState::Succeeded,
                        Err(error) => failure_state(error, &cancel),
                    };
                    job.summary_error = result.err().map(|e| failure_summary(&e));
                    if let Err(error) = repository.save_job(job.clone()).await {
                        job.state = SyncJobState::Interrupted;
                        job.summary_error = Some(format!("cannot persist terminal sync job state: {error}"));
                        let recovery = repository.save_job(job.clone()).await.err().map(|e| e.to_string());
                        release_reservation(&reserved, &job.scope).await;
                        cancellations.lock().await.remove(&job.id);
                        return Err(recovery.map_or_else(|| format!("cannot persist terminal sync job state: {error}"), |e| format!("cannot persist terminal sync job state: {error}; cannot mark it interrupted: {e}")));
                    }
                    release_reservation(&reserved, &job.scope).await;
                    cancellations.lock().await.remove(&job.id);
                }
                while let Some(Command::Run(job, cancel, _)) = receiver.recv().await {
                    cancel.cancel();
                    mark_interrupted(&engine, &repository, &job).await?;
                    release_reservation(&reserved, &job.scope).await;
                    cancellations.lock().await.remove(&job.id);
                }
                Ok(())
            }
            .await;
            if let Err(error) = &result {
                let _ = fatal_tx.send(Some(error.clone()));
                receiver.close();
                {
                    let mut state = reserved.lock().await;
                    state.accepting = false;
                    state.all = false;
                    state.chats.clear();
                    for token in cancellations.lock().await.values() {
                        token.cancel();
                    }
                }
                while let Some(Command::Run(job, cancel, _)) = receiver.recv().await {
                    cancel.cancel();
                    if let Err(cleanup) = mark_interrupted(&engine, &repository, &job).await {
                        result = Err(format!(
                            "{error}; could not interrupt queued job {}: {cleanup}",
                            job.id
                        ));
                        break;
                    }
                    cancellations.lock().await.remove(&job.id);
                }
            }
            result
        });
        *coordinator
            .worker
            .try_lock()
            .expect("worker handle mutex unexpectedly locked") = Some(worker);
        coordinator
    }

    pub async fn submit(&self, scope: SyncScope) -> Result<SyncJob, ApplicationError> {
        self.submit_with(scope, false).await
    }

    /// `refetch` re-downloads the whole history to backfill NULL metadata; chat scope only.
    pub async fn submit_with(
        &self,
        scope: SyncScope,
        refetch: bool,
    ) -> Result<SyncJob, ApplicationError> {
        if refetch && !matches!(scope, SyncScope::Chat(_)) {
            return Err(ApplicationError::Conflict);
        }
        let permit = self
            .sender
            .clone()
            .try_reserve_owned()
            .map_err(|_| ApplicationError::Busy)?;
        let mut reservations = self.reserved.lock().await;
        if !reservations.accepting {
            return Err(ApplicationError::Busy);
        }
        if let SyncScope::Chat(id) = &scope {
            match self.chats.get(*id).await? {
                None => return Err(ApplicationError::NotFound),
                Some(chat) if !chat.tracked => return Err(ApplicationError::NotTracked),
                Some(_) => {}
            }
        }
        match &scope {
            SyncScope::Chat(id) => {
                if reservations.all || reservations.chats.contains(id) {
                    return Err(ApplicationError::Conflict);
                }
                reservations.chats.insert(*id);
            }
            SyncScope::All => {
                if reservations.all || !reservations.chats.is_empty() {
                    return Err(ApplicationError::Conflict);
                }
                reservations.all = true;
            }
        }
        let id = format!(
            "sync-{}-{}",
            Utc::now().timestamp_millis(),
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let job = SyncJob {
            id,
            scope: scope.clone(),
            state: SyncJobState::Queued,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            summary_error: None,
        };
        if let Err(error) = self.repository.save_job(job.clone()).await {
            if let SyncScope::Chat(chat) = &scope {
                reservations.chats.remove(chat);
            } else {
                reservations.all = false;
            }
            return Err(error.into());
        }
        let cancel = CancellationToken::default();
        self.cancellations
            .lock()
            .await
            .insert(job.id.clone(), cancel.clone());
        permit.send(Command::Run(job.clone(), cancel, refetch));
        Ok(job)
    }

    pub async fn get_job(&self, id: &str) -> Result<SyncJob, ApplicationError> {
        self.repository
            .get_job(id)
            .await?
            .ok_or(ApplicationError::NotFound)
    }

    pub async fn cancel(&self, id: &str) -> Result<(), ApplicationError> {
        self.cancellations
            .lock()
            .await
            .get(id)
            .ok_or(ApplicationError::NotFound)?
            .cancel();
        Ok(())
    }

    pub async fn wait_job(&self, id: &str) -> Result<SyncJob, ApplicationError> {
        let mut fatal = self.fatal.clone();
        loop {
            if let Some(error) = fatal.borrow().clone() {
                return Err(ApplicationError::Internal(format!(
                    "sync worker failed: {error}"
                )));
            }
            let job = self.get_job(id).await?;
            if matches!(
                job.state,
                SyncJobState::Succeeded
                    | SyncJobState::Failed
                    | SyncJobState::Interrupted
                    | SyncJobState::RateLimited
            ) {
                return Ok(job);
            }
            tokio::select! { _ = tokio::time::sleep(Duration::from_millis(100)) => {}, changed = fatal.changed() => { if changed.is_err() { return Err(ApplicationError::Internal("sync worker stopped".into())); } } }
        }
    }

    /// Returns when the coordinator can no longer process jobs; runtimes should
    /// treat this as a critical component failure and begin graceful shutdown.
    pub async fn wait_worker_failure(&self) -> Result<(), ApplicationError> {
        let mut fatal = self.fatal.clone();
        loop {
            if let Some(error) = fatal.borrow().clone() {
                return Err(ApplicationError::Internal(format!(
                    "sync worker failed: {error}"
                )));
            }
            if fatal.changed().await.is_err() {
                return Err(ApplicationError::Internal("sync worker stopped".into()));
            }
        }
    }

    pub async fn shutdown(&self) -> Result<(), ApplicationError> {
        {
            let mut state = self.reserved.lock().await;
            state.accepting = false;
            for token in self.cancellations.lock().await.values() {
                token.cancel();
            }
            let _ = self.shutdown.send(true);
        }
        let mut worker = self.worker.lock().await;
        if let Some(task) = worker.as_mut() {
            let result = task
                .await
                .map_err(|error| {
                    ApplicationError::Internal(format!("sync worker task failed: {error}"))
                })?
                .map_err(ApplicationError::Internal);
            *worker = None;
            result?;
        }
        Ok(())
    }

    /// Stop accepting work, cancel the active job, and bound the time spent
    /// joining the worker. On timeout the task is aborted and joined so it
    /// cannot outlive the runtime; a later startup marks any persisted active
    /// job interrupted during recovery.
    pub async fn shutdown_with_timeout(&self, timeout: Duration) -> Result<(), ApplicationError> {
        {
            let mut state = self.reserved.lock().await;
            state.accepting = false;
            for token in self.cancellations.lock().await.values() {
                token.cancel();
            }
            let _ = self.shutdown.send(true);
        }

        let mut worker = self.worker.lock().await;
        let Some(task) = worker.as_mut() else {
            return Ok(());
        };
        match tokio::time::timeout(timeout, &mut *task).await {
            Ok(joined) => {
                *worker = None;
                joined
                    .map_err(|error| {
                        ApplicationError::Internal(format!("sync worker task failed: {error}"))
                    })?
                    .map_err(ApplicationError::Internal)
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                *worker = None;
                Err(ApplicationError::Internal(
                    "sync worker shutdown exceeded its deadline; active work will be recovered on restart".into(),
                ))
            }
        }
    }
}

async fn mark_interrupted(
    engine: &SyncEngine,
    repository: &Arc<dyn SyncRepository>,
    job: &SyncJob,
) -> Result<(), String> {
    let mut interrupted = job.clone();
    interrupted.state = SyncJobState::Interrupted;
    interrupted.completed_at = Some(Utc::now());
    interrupted.summary_error = Some("coordinator shut down before job started".into());
    repository
        .save_job(interrupted)
        .await
        .map_err(|e| e.to_string())?;
    let progress = repository
        .list_chat_progress(&job.id)
        .await
        .map_err(|e| e.to_string())?;
    let updates = progress
        .into_iter()
        .map(|mut item| {
            item.state = SyncJobState::Interrupted;
            item.summary_error = Some("coordinator shut down before job completed".into());
            item
        })
        .collect::<Vec<_>>();
    for update in updates {
        engine
            .sink
            .submit(IngestBatch {
                job_progress: Some(update),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn release_reservation(state: &Mutex<Reservation>, scope: &SyncScope) {
    let mut state = state.lock().await;
    match scope {
        SyncScope::Chat(id) => {
            state.chats.remove(id);
        }
        SyncScope::All => state.all = false,
    }
}

async fn run_all(
    engine: &SyncEngine,
    job_id: &str,
    cancel: &CancellationToken,
) -> Result<(), ApplicationError> {
    let chats = engine.tracked_snapshot(cancel).await?;
    let mut failures = Vec::new();
    for id in chats {
        if cancel.is_cancelled() {
            return Err(ApplicationError::Conflict);
        }
        if let Err(error) = engine.sync_chat(job_id, id, cancel).await {
            failures.push(format!("{}: {}", id.get(), failure_summary(&error)));
            let committed = engine
                .repository
                .list_chat_progress(job_id)
                .await?
                .into_iter()
                .find(|progress| progress.chat_id == id)
                .map_or(0, |progress| progress.committed_messages);
            engine
                .sink
                .submit(IngestBatch {
                    job_progress: Some(SyncChatProgress {
                        job_id: job_id.into(),
                        chat_id: id,
                        state: failure_state(&error, cancel),
                        committed_messages: committed,
                        summary_error: Some(failure_summary(&error)),
                    }),
                    ..Default::default()
                })
                .await?;
            if matches!(error, ApplicationError::TelegramFloodWait { .. }) && !cancel.is_cancelled()
            {
                // Remaining chats would hit the same limit; stop and let a later run resume.
                return Err(error);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(ApplicationError::Internal(failures.join("; ")))
    }
}

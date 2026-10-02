use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{
    ApplicationError, IngestBatch, IngestRecord, RepositoryError, TrackingScope,
    services::{CollectorStatusHandle, ComponentState, ComponentStatus},
    sync::{SyncEngine, sanitize_reason},
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RealtimeSourceError {
    /// Connection-level trouble; the supervisor catches up and starts the source again.
    #[error("transient realtime failure: {0}")]
    Transient(String),
    /// Authorization, account or storage failures; retrying cannot help.
    #[error("fatal realtime failure: {0}")]
    Fatal(String),
}

/// A live update source. `run_once` returns `Ok` only after `cancel` fired.
#[async_trait]
pub trait RealtimeSource: Send + Sync {
    async fn run_once(&self, cancel: CancellationToken) -> Result<(), RealtimeSourceError>;
}

#[derive(Debug, Clone, Copy)]
pub struct ReconnectBackoff {
    pub initial: Duration,
    pub max: Duration,
    /// A session that stayed up at least this long resets the backoff.
    pub healthy_after: Duration,
    /// While a live session runs, how often newly tracked chats are given a catch-up baseline.
    pub baseline_poll: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            healthy_after: Duration::from_secs(120),
            baseline_poll: Duration::from_secs(15),
        }
    }
}

/// Alternates a catch-up round with a live session, reconnecting with capped, cancellable
/// exponential backoff after transient failures. Returns `Ok` when cancelled.
pub async fn supervise_realtime(
    source: &dyn RealtimeSource,
    engine: &SyncEngine,
    backoff: ReconnectBackoff,
    cancel: CancellationToken,
) -> Result<(), ApplicationError> {
    let status = CollectorStatusHandle::new(ComponentStatus::disabled());
    supervise_realtime_with_status(source, engine, backoff, cancel, &status).await
}

/// Same as [`supervise_realtime`], publishing the collector state to `status`.
pub async fn supervise_realtime_with_status(
    source: &dyn RealtimeSource,
    engine: &SyncEngine,
    backoff: ReconnectBackoff,
    cancel: CancellationToken,
    status: &CollectorStatusHandle,
) -> Result<(), ApplicationError> {
    let result = supervise_inner(source, engine, backoff, cancel, status).await;
    match &result {
        Ok(()) => status.set(ComponentState::Stopped, None),
        Err(error) => status.set(
            ComponentState::Failed,
            Some(sanitize_reason(&error.to_string())),
        ),
    }
    result
}

async fn supervise_inner(
    source: &dyn RealtimeSource,
    engine: &SyncEngine,
    backoff: ReconnectBackoff,
    cancel: CancellationToken,
    status: &CollectorStatusHandle,
) -> Result<(), ApplicationError> {
    let mut delay = backoff.initial;
    let mut attempt = 0u32;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        status.set(ComponentState::CatchingUp, None);
        let mut detail = None;
        match engine.catch_up_all(&cancel).await {
            Ok(summary) => {
                if !summary.failures.is_empty() {
                    detail = Some(format!(
                        "catch-up skipped {} chat(s); retried on the next round",
                        summary.failures.len()
                    ));
                }
            }
            Err(_) if cancel.is_cancelled() => return Ok(()),
            Err(error) => return Err(error),
        }
        let started = Instant::now();
        status.set(ComponentState::Running, detail);
        match run_with_baselines(source, engine, backoff.baseline_poll, &cancel).await {
            Ok(()) => return Ok(()),
            Err(RealtimeSourceError::Fatal(message)) => {
                return Err(ApplicationError::Internal(message));
            }
            Err(RealtimeSourceError::Transient(message)) => {
                if cancel.is_cancelled() {
                    return Ok(());
                }
                if started.elapsed() >= backoff.healthy_after {
                    delay = backoff.initial;
                    attempt = 0;
                }
                attempt += 1;
                status.set(
                    ComponentState::Reconnecting,
                    Some(format!(
                        "attempt {attempt}; next retry in {} ms; last error: {}",
                        delay.as_millis(),
                        sanitize_reason(&message)
                    )),
                );
                tracing::warn!(%message, retry_in_ms = delay.as_millis() as u64, "realtime session ended; reconnecting");
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(delay) => {}
                }
                delay = (delay * 2).min(backoff.max);
            }
        }
    }
}

/// Runs the live session while periodically baselining newly tracked chats. The session future is
/// never dropped midway; the poller stops once it returns.
async fn run_with_baselines(
    source: &dyn RealtimeSource,
    engine: &SyncEngine,
    poll: Duration,
    cancel: &CancellationToken,
) -> Result<(), RealtimeSourceError> {
    let done = CancellationToken::new();
    let session = async {
        let result = source.run_once(cancel.clone()).await;
        done.cancel();
        result
    };
    let poller = async {
        loop {
            tokio::select! {
                _ = done.cancelled() => return,
                _ = tokio::time::sleep(poll) => {}
            }
            if let Err(error) = engine.baseline_new_tracked(&done).await
                && !done.is_cancelled()
            {
                tracing::warn!(reason = %sanitize_reason(&error.to_string()), "baselining newly tracked chats failed; retrying later");
            }
        }
    };
    tokio::join!(session, poller).0
}

/// Drops everything that does not belong to a tracked chat before anything is written.
///
/// Records, chats and senders of untracked chats (including channel deletions) are removed;
/// senders are kept only when a kept record references them. Account-wide (common-namespace)
/// deletions carry no chat ID, so they are kept only when they match an already archived message
/// of a tracked non-channel chat; others create no tombstone.
pub async fn restrict_to_tracked(
    scope: &dyn TrackingScope,
    mut batch: IngestBatch,
) -> Result<IngestBatch, RepositoryError> {
    let tracked = scope.tracked_chat_ids().await?;
    let record_chat = |record: &IngestRecord| match &record.event {
        crate::domain::MessageEvent::Created(message)
        | crate::domain::MessageEvent::Updated(message) => message.chat_id,
        crate::domain::MessageEvent::Deleted { chat_id, .. } => *chat_id,
    };
    batch
        .records
        .retain(|record| tracked.contains(&record_chat(record)));
    batch.chats.retain(|chat| tracked.contains(&chat.id));
    let kept_senders: std::collections::HashSet<_> = batch
        .records
        .iter()
        .filter_map(|record| match &record.event {
            crate::domain::MessageEvent::Created(message)
            | crate::domain::MessageEvent::Updated(message) => message.sender_id,
            crate::domain::MessageEvent::Deleted { .. } => None,
        })
        .collect();
    batch
        .senders
        .retain(|sender| kept_senders.contains(&sender.id));
    if !batch.account_deletions.is_empty() {
        let ids: Vec<_> = batch
            .account_deletions
            .iter()
            .map(|deletion| deletion.message_id)
            .collect();
        let archived = scope.archived_common_message_ids(&ids).await?;
        batch
            .account_deletions
            .retain(|deletion| archived.contains(&deletion.message_id));
    }
    Ok(batch)
}

/// True when nothing in the batch would be written.
pub fn is_empty(batch: &IngestBatch) -> bool {
    batch.chats.is_empty()
        && batch.senders.is_empty()
        && batch.records.is_empty()
        && batch.account_deletions.is_empty()
        && batch.checkpoint.is_none()
        && batch.job_progress.is_none()
        && batch.chat_error.is_none()
}

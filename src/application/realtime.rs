use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{
    ApplicationError,
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
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            healthy_after: Duration::from_secs(120),
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
        match source.run_once(cancel.clone()).await {
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

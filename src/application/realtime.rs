use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{ApplicationError, sync::SyncEngine};

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
    let mut delay = backoff.initial;
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        match engine.catch_up_all(&cancel).await {
            Ok(summary) => {
                for (chat, error) in &summary.failures {
                    tracing::warn!(chat_id = chat.get(), %error, "catch-up skipped a chat; it is retried on the next round");
                }
            }
            Err(_) if cancel.is_cancelled() => return Ok(()),
            Err(error) => return Err(error),
        }
        let started = Instant::now();
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
                }
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

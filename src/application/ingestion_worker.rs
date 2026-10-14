use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use super::{
    ArchiveWriter, IngestBatch, RepositoryError,
    events::{ArchiveChange, ArchiveEvents},
    services::IngestionService,
};

struct Request {
    batch: IngestBatch,
    ack: oneshot::Sender<Result<(), RepositoryError>>,
}

/// Bounded, acknowledged path shared by realtime ingestion and history sync.
#[derive(Clone)]
pub struct IngestSink {
    sender: mpsc::Sender<Request>,
}

impl IngestSink {
    pub async fn submit(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
        let (ack, result) = oneshot::channel();
        self.sender
            .send(Request { batch, ack })
            .await
            .map_err(|_| RepositoryError::Unavailable("ingestion worker stopped".into()))?;
        result.await.map_err(|_| {
            RepositoryError::Unavailable("ingestion worker stopped before acknowledgement".into())
        })?
    }
}

/// Start the sole writer task. A failed write is acknowledged as failure and
/// terminates the task, so later requests cannot silently pass the failed batch.
pub fn spawn(
    writer: Arc<dyn ArchiveWriter>,
    capacity: usize,
) -> (
    IngestSink,
    tokio::task::JoinHandle<Result<(), RepositoryError>>,
) {
    spawn_with_events(writer, capacity, None)
}

/// Like [`spawn`], publishing an [`ArchiveChange`] after each successfully committed batch.
pub fn spawn_with_events(
    writer: Arc<dyn ArchiveWriter>,
    capacity: usize,
    events: Option<ArchiveEvents>,
) -> (
    IngestSink,
    tokio::task::JoinHandle<Result<(), RepositoryError>>,
) {
    let (sender, mut receiver) = mpsc::channel::<Request>(capacity.max(1));
    let task = tokio::spawn(async move {
        let ingestion = IngestionService::new(writer);
        while let Some(request) = receiver.recv().await {
            let change = events
                .as_ref()
                .and_then(|_| ArchiveChange::from_batch(&request.batch));
            match ingestion.ingest_batch(request.batch).await {
                Ok(()) => {
                    if let (Some(events), Some(change)) = (&events, change) {
                        events.publish(change);
                    }
                    let _ = request.ack.send(Ok(()));
                }
                Err(error) => {
                    let message = error.to_string();
                    let repository_error = match error {
                        crate::application::ApplicationError::RepositoryUnavailable(message)
                        | crate::application::ApplicationError::Internal(message) => {
                            RepositoryError::Unavailable(message)
                        }
                        other => RepositoryError::Unavailable(other.to_string()),
                    };
                    let _ = request.ack.send(Err(repository_error));
                    receiver.close();
                    while let Some(pending) = receiver.recv().await {
                        let _ = pending.ack.send(Err(RepositoryError::Unavailable(format!(
                            "ingestion worker failed: {message}"
                        ))));
                    }
                    return Err(RepositoryError::Unavailable(message));
                }
            }
        }
        Ok(())
    });
    (IngestSink { sender }, task)
}

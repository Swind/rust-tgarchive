use std::sync::Arc;

use async_trait::async_trait;
use tgarchive::application::{ArchiveWriter, IngestBatch, RepositoryError, ingestion_worker};
use tokio::sync::Notify;

struct GateWriter {
    started: Notify,
    release: Notify,
}

#[async_trait]
impl ArchiveWriter for GateWriter {
    async fn write_batch(&self, _: IngestBatch) -> Result<(), RepositoryError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok(())
    }
}

struct FailWriter;
#[async_trait]
impl ArchiveWriter for FailWriter {
    async fn write_batch(&self, _: IngestBatch) -> Result<(), RepositoryError> {
        Err(RepositoryError::Unavailable("disk full".into()))
    }
}

#[tokio::test]
async fn acknowledgement_waits_for_commit_and_queue_is_bounded() {
    let writer = Arc::new(GateWriter {
        started: Notify::new(),
        release: Notify::new(),
    });
    let (sink, worker) = ingestion_worker::spawn(writer.clone(), 1);
    let first = {
        let sink = sink.clone();
        tokio::spawn(async move { sink.submit(IngestBatch::default()).await })
    };
    writer.started.notified().await;
    assert!(!first.is_finished());
    let second = {
        let sink = sink.clone();
        tokio::spawn(async move { sink.submit(IngestBatch::default()).await })
    };
    tokio::task::yield_now().await;
    let third = {
        let sink = sink.clone();
        tokio::spawn(async move { sink.submit(IngestBatch::default()).await })
    };
    tokio::task::yield_now().await;
    assert!(!third.is_finished());
    writer.release.notify_one();
    assert!(first.await.unwrap().is_ok());
    writer.started.notified().await;
    writer.release.notify_one();
    assert!(second.await.unwrap().is_ok());
    writer.started.notified().await;
    writer.release.notify_one();
    assert!(third.await.unwrap().is_ok());
    drop(sink);
    assert!(worker.await.unwrap().is_ok());
}

#[tokio::test]
async fn writer_failure_is_returned_and_worker_stops() {
    let (sink, worker) = ingestion_worker::spawn(Arc::new(FailWriter), 1);
    let error = sink.submit(IngestBatch::default()).await.unwrap_err();
    assert!(error.to_string().contains("disk full"));
    assert!(worker.await.unwrap().is_err());
    assert!(sink.submit(IngestBatch::default()).await.is_err());
}

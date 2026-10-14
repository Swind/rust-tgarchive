use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tgarchive::{
    application::{
        AccountDeletion, ArchiveWriter, ChatRepository, IngestBatch, IngestRecord, MessageSource,
        RepositoryError,
        events::{ArchiveChange, ArchiveEvents},
        ingestion_worker,
        services::{Application, ComponentState, ComponentStatus},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
    infrastructure::persistence::sqlite::SqliteStore,
    interface::rest::router_with_events,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Duration, timeout},
};
use tower::ServiceExt;

fn chat(id: i64) -> ChatId {
    ChatId::from_marked(id).unwrap()
}

fn message(chat_id: i64, id: i64) -> Message {
    let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    Message {
        id: MessageId::new(id).unwrap(),
        chat_id: chat(chat_id),
        sender_id: None,
        timestamp: now,
        edited_at: None,
        collected_at: now,
        text: Some("x".into()),
        reply_to: None,
        attachments: vec![],
        post_author: None,
        forward: None,
    }
}

fn record(event: MessageEvent, source: MessageSource) -> IngestRecord {
    IngestRecord { event, source }
}

/// Records whether events were already visible while the write ran.
struct Writer {
    probe: Mutex<tokio::sync::broadcast::Receiver<ArchiveChange>>,
    fail: bool,
    seen_during_write: Mutex<Vec<bool>>,
}

#[async_trait]
impl ArchiveWriter for Writer {
    async fn write_batch(&self, _: IngestBatch) -> Result<(), RepositoryError> {
        let seen = self.probe.lock().unwrap().try_recv().is_ok();
        self.seen_during_write.lock().unwrap().push(seen);
        if self.fail {
            Err(RepositoryError::Unavailable("boom".into()))
        } else {
            Ok(())
        }
    }
}

fn writer(fail: bool) -> (Arc<Writer>, ArchiveEvents) {
    let events = ArchiveEvents::new(8);
    (
        Arc::new(Writer {
            probe: Mutex::new(events.subscribe()),
            fail,
            seen_during_write: Mutex::default(),
        }),
        events,
    )
}

#[tokio::test]
async fn publishes_deduplicated_chats_after_commit_for_every_source() {
    let (writer, events) = writer(false);
    let mut rx = events.subscribe();
    let (sink, _task) = ingestion_worker::spawn_with_events(writer.clone(), 4, Some(events));
    let deleted = MessageEvent::Deleted {
        chat_id: chat(-3),
        message_id: MessageId::new(1).unwrap(),
        deleted_at: Utc::now(),
    };
    sink.submit(IngestBatch {
        chats: vec![],
        records: vec![
            record(
                MessageEvent::Created(message(-2, 1)),
                MessageSource::History,
            ),
            record(
                MessageEvent::Updated(message(-2, 2)),
                MessageSource::Realtime,
            ),
            record(
                MessageEvent::Created(message(-1, 3)),
                MessageSource::Refetch,
            ),
            record(deleted, MessageSource::Realtime),
        ],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        ArchiveChange {
            chat_ids: vec![-3, -2, -1],
            global: false
        }
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(*writer.seen_during_write.lock().unwrap(), [false]);
}

#[tokio::test]
async fn account_wide_deletion_is_global_and_empty_batches_are_silent() {
    let (writer, events) = writer(false);
    let mut rx = events.subscribe();
    let (sink, _task) = ingestion_worker::spawn_with_events(writer, 4, Some(events));
    sink.submit(IngestBatch::default()).await.unwrap();
    assert!(rx.try_recv().is_err());
    sink.submit(IngestBatch {
        account_deletions: vec![AccountDeletion {
            message_id: MessageId::new(5).unwrap(),
            deleted_at: Utc::now(),
        }],
        ..Default::default()
    })
    .await
    .unwrap();
    let change = rx.try_recv().unwrap();
    assert!(change.global && change.chat_ids.is_empty());
}

#[tokio::test]
async fn failed_write_publishes_nothing() {
    let (writer, events) = writer(true);
    let mut rx = events.subscribe();
    let (sink, _task) = ingestion_worker::spawn_with_events(writer, 4, Some(events));
    let result = sink
        .submit(IngestBatch {
            records: vec![record(
                MessageEvent::Created(message(-2, 1)),
                MessageSource::History,
            )],
            ..Default::default()
        })
        .await;
    assert!(result.is_err());
    assert!(rx.try_recv().is_err());
}

async fn router(events: Option<ArchiveEvents>) -> (tempfile::TempDir, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&format!("sqlite://{}", dir.path().join("e.db").display()))
        .await
        .unwrap();
    store
        .save_refresh(vec![Chat {
            id: chat(-10001),
            kind: ChatKind::Supergroup,
            title: None,
            username: None,
            tracked: true,
        }])
        .await
        .unwrap();
    let ports = Arc::new(store);
    let app = Arc::new(Application::new(
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports,
        None,
        ComponentStatus::new(ComponentState::Running, None),
    ));
    (dir, router_with_events(app, None, None, events))
}

#[tokio::test]
async fn unavailable_without_collector() {
    let (_dir, router) = router(None).await;
    let response = router
        .oneshot(Request::get("/api/v1/events").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], json!("events_unavailable"));
}

async fn read_until(stream: &mut TcpStream, buffer: &mut String, needle: &str) {
    timeout(Duration::from_secs(5), async {
        let mut chunk = [0u8; 1024];
        while !buffer.contains(needle) {
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "stream closed before {needle:?}; got {buffer:?}");
            buffer.push_str(&String::from_utf8_lossy(&chunk[..read]));
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got {buffer:?}"));
}

#[tokio::test]
async fn streams_headers_resync_changes_and_closes_before_graceful_shutdown() {
    let events = ArchiveEvents::new(8);
    let (_dir, router) = router(Some(events.clone())).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let closer = events.clone();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = stopped.await;
                closer.close();
            })
            .await
    });
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /api/v1/events HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n")
        .await
        .unwrap();
    let mut buffer = String::new();
    read_until(&mut stream, &mut buffer, "event: resync\ndata: {}\n\n").await;
    let head = buffer.to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 200"));
    assert!(head.contains("content-type: text/event-stream"));
    assert!(head.contains("cache-control: no-cache"));
    assert!(head.contains("x-accel-buffering: no"));
    events.publish(ArchiveChange {
        chat_ids: vec![-10001],
        global: false,
    });
    read_until(
        &mut stream,
        &mut buffer,
        "event: archive_changed\ndata: {\"chat_ids\":[-10001],\"global\":false}\n\n",
    )
    .await;
    // The connection is still open; graceful shutdown must end it and finish.
    stop.send(()).unwrap();
    timeout(Duration::from_secs(5), server)
        .await
        .expect("graceful shutdown hung on open SSE stream")
        .unwrap()
        .unwrap();
}

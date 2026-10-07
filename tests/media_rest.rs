use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tgarchive::{
    application::{
        ArchiveWriter, ChatRepository, IngestBatch, IngestRecord, MessageSource,
        services::{Application, ComponentState, ComponentStatus},
    },
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId,
    },
    infrastructure::persistence::sqlite::{SqliteStore, media::MediaPolicy},
    interface::rest::{MediaContext, router_with_media},
};
use tower::ServiceExt;

async fn setup(worker_enabled: bool) -> (tempfile::TempDir, SqliteStore, ChatId, axum::Router) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&format!(
        "sqlite://{}",
        dir.path().join("media.db").display()
    ))
    .await
    .unwrap();
    let chat = ChatId::from_marked(-10001).unwrap();
    let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    store
        .save_refresh(vec![Chat {
            id: chat,
            kind: ChatKind::Supergroup,
            title: Some("media".into()),
            username: None,
            tracked: true,
        }])
        .await
        .unwrap();
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    store
        .set_media_policy(chat.get(), MediaPolicy::default())
        .await
        .unwrap();
    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Created(Message {
                    post_author: None,
                    forward: None,
                    id: MessageId::new(8).unwrap(),
                    chat_id: chat,
                    sender_id: None,
                    timestamp: now,
                    edited_at: None,
                    collected_at: now,
                    text: None,
                    reply_to: None,
                    attachments: vec![Attachment {
                        kind: AttachmentKind::Photo,
                        telegram_file_id: Some("file-photo".into()),
                        mime_type: Some("image/png".into()),
                        file_name: None,
                        size: Some(3),
                    }],
                }),
                source: MessageSource::Realtime,
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let ports = Arc::new(store.clone());
    let app = Arc::new(Application::new(
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports,
        None,
        ComponentStatus::new(ComponentState::Running, None),
    ));
    let router = router_with_media(
        app,
        None,
        Some(MediaContext {
            store: store.clone(),
            media_dir: dir.path().join("media"),
            worker_enabled,
        }),
    );
    (dir, store, chat, router)
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(path);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(Body::from(body.map_or_else(String::new, |b| b.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, bytes)
}

#[tokio::test]
async fn preview_reads_are_local_archive_is_idempotent_and_content_stays_inside_media_dir() {
    let (dir, store, chat, app) = setup(true).await;
    let path = format!("/api/v1/chats/{}/messages/8/media", chat.get());
    let (status, media, _) = call(&app, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let preview_id = media[0]["id"].as_i64().unwrap();
    let preview = store.get_media(preview_id).await.unwrap().unwrap();
    assert_eq!(
        preview.attempts, 0,
        "GET must not start a Telegram download"
    );
    let (_, archive, _) = call(
        &app,
        "POST",
        &format!("/api/v1/media/{preview_id}/archive"),
        None,
    )
    .await;
    let archive_id = archive["id"].as_i64().unwrap();
    let (_, again, _) = call(
        &app,
        "POST",
        &format!("/api/v1/media/{preview_id}/archive"),
        None,
    )
    .await;
    assert_eq!(again["id"], archive_id);

    tokio::fs::create_dir_all(dir.path().join("media"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("media/ok.png"), b"png")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("outside.png"), b"secret")
        .await
        .unwrap();
    let job = store.claim_media().await.unwrap().unwrap();
    store
        .finish_media(job.id, "ok.png", "image/png", 3, Some(1), Some(1))
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/media/{}/content", job.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes.as_ref(), b"png");

    let escaping = store.claim_media().await.unwrap().unwrap();
    store
        .finish_media(escaping.id, "../outside.png", "image/png", 6, None, None)
        .await
        .unwrap();
    let (status, _, _) = call(
        &app,
        "GET",
        &format!("/api/v1/media/{}/content", escaping.id),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "content cannot escape media_dir"
    );

    let now = DateTime::<Utc>::from_timestamp(1_700_000_100, 0).unwrap();
    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Deleted {
                    chat_id: chat,
                    message_id: MessageId::new(8).unwrap(),
                    deleted_at: now,
                },
                source: MessageSource::Realtime,
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let (status, _, _) = call(
        &app,
        "GET",
        &format!("/api/v1/media/{}/content", job.id),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, empty, _) = call(&app, "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, json!([]));
}

#[tokio::test]
async fn download_mutations_report_missing_worker_and_bad_json_keeps_error_envelope() {
    let (_dir, store, chat, app) = setup(false).await;
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/chats/{}/media/downloads", chat.get()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, media, _) = call(
        &app,
        "GET",
        &format!("/api/v1/chats/{}/messages/8/media", chat.get()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        store
            .get_media(media[0]["id"].as_i64().unwrap())
            .await
            .unwrap()
            .unwrap()
            .attempts,
        0
    );
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/media/{}/archive", media[0]["id"].as_i64().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/v1/chats/{}/media-policy", chat.get()))
                .header("content-type", "application/json")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let envelope: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["error"]["code"], "malformed_request");
    assert!(envelope["error"]["request_id"].as_str().is_some());

    let (status, _, _) = call(&app, "GET", "/api/v1/chats/-10002/media-policy", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn media_progress_is_readable_without_worker_and_counts_only_current_sources() {
    let (_dir, store, chat, app) = setup(false).await;
    let path = "/api/v1/media/downloads/status";
    let (status, rows, _) = call(&app, "GET", path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rows[0]["queued"], 1);
    assert_eq!(rows[0]["total"], 1);
    assert_eq!(rows[0]["title"], "media");
    let job = store.claim_media().await.unwrap().unwrap();
    let (_, rows, _) = call(&app, "GET", path, None).await;
    assert_eq!(rows[0]["running"], 1);
    store
        .fail_media(job.id, "failed", "rate limited", Some(i64::MAX))
        .await
        .unwrap();
    let (_, rows, _) = call(&app, "GET", path, None).await;
    assert_eq!(rows[0]["failed"], 1);
    assert_eq!(rows[0]["retrying"], 1);
    for state in ["failed", "interrupted", "unavailable", "superseded"] {
        store.fail_media(job.id, state, "test", None).await.unwrap();
        let (_, rows, _) = call(&app, "GET", path, None).await;
        assert_eq!(rows[0][state], 1);
        assert_eq!(rows[0]["retrying"], 0);
        assert_eq!(rows[0]["total"], 1);
    }
    store.request_archive(job.id).await.unwrap();
    let (_, rows, _) = call(&app, "GET", path, None).await;
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(rows[0]["variant"], "archive");
    assert_eq!(rows[0]["queued"], 1);
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        _dir.path().join("media.db").display()
    ))
    .await
    .unwrap();
    sqlx::query("UPDATE attachments SET telegram_file_id='replacement' WHERE message_row_id=(SELECT row_id FROM messages WHERE chat_id=? AND message_id=8)").bind(chat.get()).execute(&pool).await.unwrap();
    let (_, rows, _) = call(&app, "GET", path, None).await;
    assert_eq!(rows, json!([]));
    sqlx::query("UPDATE attachments SET telegram_file_id='file-photo'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE messages SET is_deleted=1")
        .execute(&pool)
        .await
        .unwrap();
    let (_, rows, _) = call(&app, "GET", path, None).await;
    assert_eq!(rows, json!([]));
}

//! Read-side API for the web UI: sender names, deleted messages, chat stats, message context
//! and dev CORS, exercised against a real SQLite archive through the REST router.

use std::{path::Path, process::Command, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{DateTime, SecondsFormat};
use serde_json::Value;
use tgarchive::{
    application::{
        ArchiveWriter, ChatCheckpoint, IngestBatch, IngestRecord, MessageSource,
        services::{Application, ComponentState, ComponentStatus},
    },
    domain::{
        Chat, ChatId, ChatKind, Message, MessageEvent, MessageId, Sender, SenderId, SenderKind,
    },
    infrastructure::persistence::sqlite::SqliteStore,
    interface::rest::{apply_dev_cors, dev_cors_layer, parse_dev_cors_origin, router},
};
use tower::ServiceExt;

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");
const BASE: i64 = 1_700_000_000;

fn group(raw: i64) -> ChatId {
    ChatId::from_telegram(ChatKind::Group, raw).unwrap()
}

fn user(raw: i64) -> SenderId {
    SenderId::from_telegram(SenderKind::User, raw).unwrap()
}

fn chat(id: ChatId, title: Option<&str>) -> Chat {
    Chat {
        id,
        kind: ChatKind::Group,
        title: title.map(str::to_owned),
        username: None,
        tracked: false,
    }
}

fn msg(chat_id: ChatId, id: i64, at: i64, sender: Option<SenderId>) -> Message {
    let time = DateTime::from_timestamp(at, 0).unwrap();
    Message {
        post_author: None,
        forward: None,
        id: MessageId::new(id).unwrap(),
        chat_id,
        sender_id: sender,
        timestamp: time,
        edited_at: None,
        collected_at: time,
        text: Some(format!("body {id}")),
        reply_to: None,
        attachments: vec![],
    }
}

fn created(message: Message) -> IngestRecord {
    IngestRecord {
        event: MessageEvent::Created(message),
        source: MessageSource::History,
    }
}

fn deleted(chat_id: ChatId, id: i64, at: i64) -> IngestRecord {
    IngestRecord {
        event: MessageEvent::Deleted {
            chat_id,
            message_id: MessageId::new(id).unwrap(),
            deleted_at: DateTime::from_timestamp(at, 0).unwrap(),
        },
        source: MessageSource::Realtime,
    }
}

fn rfc3339(seconds: i64) -> String {
    DateTime::from_timestamp(seconds, 0)
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<SqliteStore>,
    application: Arc<Application>,
    app: axum::Router,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let application = application(&store);
    Fixture {
        dir,
        store,
        app: router(application.clone()),
        application,
    }
}

fn application(store: &Arc<SqliteStore>) -> Arc<Application> {
    Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        None,
        ComponentStatus::new(ComponentState::Running, None),
    ))
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn ids(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_i64().unwrap())
        .collect()
}

/// Chat with messages 1..=10 (timestamp BASE+id); 4 and 7 are deleted; 11 only has a tombstone.
async fn seed_with_deletions(f: &Fixture) -> ChatId {
    let chat_id = group(10);
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(chat_id, Some("Team"))],
            senders: vec![
                Sender {
                    id: user(1),
                    kind: SenderKind::User,
                    display_name: Some("Alice A".into()),
                    username: Some("alice".into()),
                    is_bot: None,
                },
                Sender {
                    id: user(2),
                    kind: SenderKind::User,
                    display_name: Some("Bob".into()),
                    username: None,
                    is_bot: None,
                },
            ],
            records: (1..=10)
                .map(|id| {
                    let sender = match id {
                        1..=6 => Some(user(1)),
                        7..=9 => Some(user(2)),
                        _ => None,
                    };
                    created(msg(chat_id, id, BASE + id, sender))
                })
                .chain([
                    deleted(chat_id, 4, BASE + 100),
                    deleted(chat_id, 7, BASE + 101),
                    deleted(chat_id, 11, BASE + 102),
                ])
                .collect(),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    chat_id
}

#[tokio::test]
async fn messages_expose_resolved_sender_and_senders_endpoint_counts_by_activity() {
    let f = fixture().await;
    let chat_id = seed_with_deletions(&f).await;
    let (status, page) = get(&f.app, &format!("/api/v1/chats/{}/messages", chat_id.get())).await;
    assert_eq!(status, StatusCode::OK);
    let items = page["items"].as_array().unwrap();
    let by_id = |id: i64| items.iter().find(|m| m["id"] == id).unwrap();
    assert_eq!(
        by_id(1)["sender"],
        serde_json::json!({"id": user(1).get(), "display_name": "Alice A", "username": "alice", "is_bot": null})
    );
    assert_eq!(by_id(8)["sender"]["display_name"], "Bob");
    assert!(by_id(8)["sender"]["username"].is_null());
    assert!(by_id(10)["sender"].is_null());
    assert!(by_id(10)["sender_id"].is_null());

    let (status, senders) = get(&f.app, &format!("/api/v1/chats/{}/senders", chat_id.get())).await;
    assert_eq!(status, StatusCode::OK);
    // Deleted messages (4, 7) are not counted: Alice 5 (1,2,3,5,6), Bob 2 (8,9).
    assert_eq!(
        senders,
        serde_json::json!([
            {"id": user(1).get(), "display_name": "Alice A", "username": "alice", "is_bot": null, "message_count": 5},
            {"id": user(2).get(), "display_name": "Bob", "username": null, "is_bot": null, "message_count": 2},
        ])
    );
    let (_, limited) = get(
        &f.app,
        &format!("/api/v1/chats/{}/senders?limit=1", chat_id.get()),
    )
    .await;
    assert_eq!(limited.as_array().unwrap().len(), 1);
    let (status, _) = get(&f.app, "/api/v1/chats/-999/senders").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(
        &f.app,
        &format!("/api/v1/chats/{}/senders?limit=0", chat_id.get()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn channel_self_posts_use_the_chat_title_and_unknown_senders_stay_resolvable() {
    let f = fixture().await;
    let channel = ChatId::from_telegram(ChatKind::Channel, 77).unwrap();
    let as_channel = SenderId::from_marked(channel.get()).unwrap();
    f.store
        .write_batch(IngestBatch {
            chats: vec![Chat {
                kind: ChatKind::Channel,
                ..chat(channel, Some("News"))
            }],
            // No `senders` row for the channel nor for user 9.
            records: vec![
                created(msg(channel, 1, BASE, Some(as_channel))),
                created(msg(channel, 2, BASE + 1, Some(user(9)))),
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let (_, one) = get(
        &f.app,
        &format!("/api/v1/chats/{}/messages/1", channel.get()),
    )
    .await;
    assert_eq!(one["sender"]["display_name"], "News");
    let (_, two) = get(
        &f.app,
        &format!("/api/v1/chats/{}/messages/2", channel.get()),
    )
    .await;
    assert_eq!(two["sender"]["id"], user(9).get());
    assert!(two["sender"]["display_name"].is_null());
}

#[tokio::test]
async fn include_deleted_is_opt_in_on_list_chat_list_search_and_get() {
    let f = fixture().await;
    let chat_id = seed_with_deletions(&f).await;
    let c = chat_id.get();

    for (path, with_deleted) in [
        (format!("/api/v1/messages?chat_id={c}"), false),
        (format!("/api/v1/chats/{c}/messages"), false),
        (format!("/api/v1/messages/search?q=body&chat_id={c}"), false),
        (
            format!("/api/v1/messages?chat_id={c}&include_deleted=false"),
            false,
        ),
        (
            format!("/api/v1/messages?chat_id={c}&include_deleted=true"),
            true,
        ),
        (
            format!("/api/v1/chats/{c}/messages?include_deleted=true"),
            true,
        ),
        (
            format!("/api/v1/messages/search?q=body&chat_id={c}&include_deleted=true"),
            true,
        ),
    ] {
        let (status, page) = get(&f.app, &path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let mut got = ids(&page["items"]);
        got.sort();
        let expected: Vec<i64> = if with_deleted {
            (1..=10).collect()
        } else {
            vec![1, 2, 3, 5, 6, 8, 9, 10]
        };
        assert_eq!(got, expected, "{path}");
        for item in page["items"].as_array().unwrap() {
            let is_deleted = item["id"] == 4 || item["id"] == 7;
            assert_eq!(item["is_deleted"], is_deleted, "{path}");
            assert_eq!(item["deleted_at"].is_null(), !is_deleted, "{path}");
        }
    }

    let (status, _) = get(&f.app, &format!("/api/v1/chats/{c}/messages/4")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = get(
        &f.app,
        &format!("/api/v1/chats/{c}/messages/4?include_deleted=true"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["is_deleted"], true);
    assert_eq!(body["deleted_at"], rfc3339(BASE + 100));
    assert_eq!(body["text"], "body 4");
    let (_, live) = get(&f.app, &format!("/api/v1/chats/{c}/messages/1")).await;
    assert_eq!(live["is_deleted"], false);
    assert!(live["deleted_at"].is_null());
    // A tombstone without a stored body is never returned.
    let (status, _) = get(
        &f.app,
        &format!("/api/v1/chats/{c}/messages/11?include_deleted=true"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(
        &f.app,
        &format!("/api/v1/chats/{c}/messages/1?include_deleted=maybe"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

async fn set_sync_completed(f: &Fixture, chat_id: ChatId, at: i64) {
    let options = <sqlx::sqlite::SqliteConnectOptions as std::str::FromStr>::from_str(&format!(
        "sqlite://{}",
        f.dir.path().join("archive.db").display()
    ))
    .unwrap();
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("UPDATE chat_sync_state SET last_sync_completed_at=? WHERE chat_id=?")
        .bind(at)
        .bind(chat_id.get())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn chat_stats_are_aggregated_in_list_and_get_and_sortable() {
    let f = fixture().await;
    let busy = seed_with_deletions(&f).await;
    let quiet = group(20);
    let empty = group(30);
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(quiet, Some("alpha")), chat(empty, None)],
            records: vec![created(msg(quiet, 1, BASE + 500, None))],
            checkpoint: Some((
                busy,
                ChatCheckpoint {
                    history_before_id: None,
                    history_complete: true,
                    catchup_after_id: None,
                },
            )),
            chat_error: Some((quiet, "flood wait".into())),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    set_sync_completed(&f, busy, BASE + 900).await;

    let (status, one) = get(&f.app, &format!("/api/v1/chats/{}", busy.get())).await;
    assert_eq!(status, StatusCode::OK);
    let stats = &one["stats"];
    assert_eq!(stats["message_count"], 8);
    assert_eq!(stats["deleted_count"], 2);
    assert_eq!(stats["first_message_at"], rfc3339(BASE + 1));
    assert_eq!(stats["last_message_at"], rfc3339(BASE + 10));
    assert_eq!(stats["history_complete"], true);
    assert_eq!(stats["last_sync_completed_at"], rfc3339(BASE + 900));
    assert!(stats["last_error"].is_null());

    let chat_ids = |v: &Value| -> Vec<i64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_i64().unwrap())
            .collect()
    };
    let (_, list) = get(&f.app, "/api/v1/chats").await;
    assert_eq!(chat_ids(&list), vec![empty.get(), quiet.get(), busy.get()]);
    let by = |id: ChatId| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id.get())
            .unwrap()
    };
    assert_eq!(by(busy)["stats"], one["stats"]);
    assert_eq!(by(quiet)["stats"]["message_count"], 1);
    assert_eq!(by(quiet)["stats"]["deleted_count"], 0);
    assert_eq!(by(quiet)["stats"]["history_complete"], false);
    assert_eq!(by(quiet)["stats"]["last_error"], "flood wait");
    assert_eq!(by(empty)["stats"]["message_count"], 0);
    assert!(by(empty)["stats"]["first_message_at"].is_null());
    assert!(by(empty)["stats"]["last_message_at"].is_null());

    let (_, by_title) = get(&f.app, "/api/v1/chats?sort=title").await;
    assert_eq!(
        chat_ids(&by_title),
        vec![quiet.get(), busy.get(), empty.get()]
    );
    let (_, by_last) = get(&f.app, "/api/v1/chats?sort=last_message").await;
    assert_eq!(
        chat_ids(&by_last),
        vec![quiet.get(), busy.get(), empty.get()]
    );
    let (_, by_count) = get(&f.app, "/api/v1/chats?sort=message_count").await;
    assert_eq!(
        chat_ids(&by_count),
        vec![busy.get(), quiet.get(), empty.get()]
    );
    let (status, _) = get(&f.app, "/api/v1/chats?sort=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Tracking responses carry stats too.
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/chats/{}/tracking", busy.get()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let tracked: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(tracked["tracked"], true);
    assert_eq!(tracked["stats"]["message_count"], 8);
}

#[tokio::test]
async fn context_returns_ascending_neighbours_with_cursors_the_list_endpoint_accepts() {
    let f = fixture().await;
    let chat_id = group(40);
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(chat_id, Some("Ctx"))],
            records: (1..=30)
                .map(|id| created(msg(chat_id, id, BASE + id, None)))
                .chain([
                    deleted(chat_id, 14, BASE + 200),
                    deleted(chat_id, 16, BASE + 201),
                ])
                .collect(),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let c = chat_id.get();
    let ctx_uri = |id: i64, query: &str| format!("/api/v1/chats/{c}/messages/{id}/context{query}");

    let (status, ctx) = get(&f.app, &ctx_uri(15, "?before=3&after=2")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctx["anchor"]["id"], 15);
    assert_eq!(ids(&ctx["before"]), vec![11, 12, 13]); // 14 is deleted and hidden
    assert_eq!(ids(&ctx["after"]), vec![17, 18]);
    assert_eq!(ctx["has_more_before"], true);
    assert_eq!(ctx["has_more_after"], true);

    // Cursors continue through the list endpoint without gaps or overlap.
    let before_cursor = ctx["before_cursor"].as_str().unwrap();
    let (_, older) = get(
        &f.app,
        &format!("/api/v1/chats/{c}/messages?before={before_cursor}&limit=2"),
    )
    .await;
    assert_eq!(ids(&older["items"]), vec![10, 9]);
    let after_cursor = ctx["after_cursor"].as_str().unwrap();
    let (_, newer) = get(
        &f.app,
        &format!("/api/v1/chats/{c}/messages?after={after_cursor}&limit=2"),
    )
    .await;
    let mut newer_ids = ids(&newer["items"]);
    newer_ids.sort();
    assert_eq!(newer_ids, vec![19, 20]);

    let (_, with) = get(
        &f.app,
        &ctx_uri(15, "?before=3&after=2&include_deleted=true"),
    )
    .await;
    assert_eq!(ids(&with["before"]), vec![12, 13, 14]);
    assert_eq!(ids(&with["after"]), vec![16, 17]);
    assert_eq!(with["before"][2]["is_deleted"], true);

    // Defaults are 20/20; edges report no more and no cursor.
    let (_, def) = get(&f.app, &ctx_uri(25, "")).await;
    assert_eq!(def["before"].as_array().unwrap().len(), 20);
    assert_eq!(def["after"].as_array().unwrap().len(), 5);
    assert_eq!(def["has_more_after"], false);
    assert!(def["after_cursor"].is_null());
    let (_, edge) = get(&f.app, &ctx_uri(1, "?before=5&after=0")).await;
    assert!(ids(&edge["before"]).is_empty());
    assert_eq!(edge["has_more_before"], false);
    assert!(edge["before_cursor"].is_null());
    assert!(ids(&edge["after"]).is_empty());
    assert_eq!(edge["has_more_after"], false);

    // Limits and 404s.
    let (status, _) = get(&f.app, &ctx_uri(15, "?before=100&after=100")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, err) = get(&f.app, &ctx_uri(15, "?before=101")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["code"], "invalid_context_size");
    let (status, _) = get(&f.app, &ctx_uri(15, "?after=101")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get(&f.app, &ctx_uri(999, "")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&f.app, &ctx_uri(14, "")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = get(&f.app, &ctx_uri(14, "?include_deleted=true")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["anchor"]["is_deleted"], true);
    let (status, _) = get(&f.app, "/api/v1/chats/-4040/messages/1/context").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn context_orders_same_second_messages_by_message_id() {
    let f = fixture().await;
    let chat_id = group(41);
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(chat_id, None)],
            records: (1..=5)
                .map(|id| created(msg(chat_id, id, BASE, None)))
                .collect(),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let uri = format!(
        "/api/v1/chats/{}/messages/3/context?before=5&after=5",
        chat_id.get()
    );
    let (_, ctx) = get(&f.app, &uri).await;
    assert_eq!(ids(&ctx["before"]), vec![1, 2]);
    assert_eq!(ids(&ctx["after"]), vec![4, 5]);
}

async fn call(app: &axum::Router, request: Request<Body>) -> (StatusCode, axum::http::HeaderMap) {
    let response = app.clone().oneshot(request).await.unwrap();
    (response.status(), response.headers().clone())
}

fn preflight(origin: &str) -> Request<Body> {
    Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/chats")
        .header("origin", origin)
        .header("access-control-request-method", "PUT")
        .header("access-control-request-headers", "content-type")
        .body(Body::empty())
        .unwrap()
}

fn simple(origin: &str) -> Request<Body> {
    Request::builder()
        .uri("/health/live")
        .header("origin", origin)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn dev_cors_allows_exactly_the_configured_origin_and_is_off_by_default() {
    let f = fixture().await;
    let origin = "http://127.0.0.1:5173";
    let on = apply_dev_cors(
        router(f.application.clone()),
        Some(dev_cors_layer(parse_dev_cors_origin(origin).unwrap())),
    );
    let (status, headers) = call(&on, preflight(origin)).await;
    assert!(status.is_success(), "{status}");
    assert_eq!(headers["access-control-allow-origin"], origin);
    assert!(
        headers["access-control-allow-methods"]
            .to_str()
            .unwrap()
            .contains("PUT")
    );
    for other in ["http://127.0.0.1:5174", "http://evil.example"] {
        let (_, headers) = call(&on, preflight(other)).await;
        assert!(
            headers.get("access-control-allow-origin").is_none(),
            "{other}"
        );
        let (_, headers) = call(&on, simple(other)).await;
        assert!(
            headers.get("access-control-allow-origin").is_none(),
            "{other}"
        );
    }
    let (status, headers) = call(&on, simple(origin)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["access-control-allow-origin"], origin);

    let off = apply_dev_cors(router(f.application.clone()), None);
    let (status, headers) = call(&off, simple(origin)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("access-control-allow-origin").is_none());
    let (_, headers) = call(&off, preflight(origin)).await;
    assert!(headers.get("access-control-allow-origin").is_none());
}

#[test]
fn dev_cors_origin_must_be_a_bare_loopback_http_origin() {
    for ok in [
        "http://127.0.0.1:5173",
        "http://localhost:5173",
        "http://localhost",
        "http://[::1]:3000",
        "http://127.0.0.1",
    ] {
        assert!(parse_dev_cors_origin(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "*",
        "https://127.0.0.1:5173",
        "http://example.com",
        "http://192.168.1.5:5173",
        "http://0.0.0.0:5173",
        "http://127.0.0.1:5173/",
        "http://127.0.0.1:5173/path",
        "http://127.0.0.1:abc",
        "http://127.0.0.1:99999",
        "http://localhost.evil.com",
        "127.0.0.1:5173",
        "http://user@127.0.0.1:5173",
    ] {
        assert!(parse_dev_cors_origin(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn serve_refuses_to_start_with_an_invalid_dev_cors_origin() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env(
            "DATABASE_URL",
            format!("sqlite://{}", dir.path().join("a.db").display()),
        )
        .env("TGARCHIVE_DEV_CORS_ORIGIN", "http://example.com:5173")
        .args(["serve", "--bind", "127.0.0.1:0", "--query-only"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("TGARCHIVE_DEV_CORS_ORIGIN"), "{stderr}");
}

#[tokio::test]
async fn migration_0005_upgrades_a_0004_database_without_touching_data() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("old.db").display());
    let old = dir.path().join("old-migrations");
    std::fs::create_dir(&old).unwrap();
    for name in [
        "0001_archive.sql",
        "0002_telegram_account_deletions.sql",
        "0003_chat_tracking.sql",
        "0004_sync_rate_limited_state.sql",
    ] {
        std::fs::copy(Path::new("migrations").join(name), old.join(name)).unwrap();
    }
    let options = <sqlx::sqlite::SqliteConnectOptions as std::str::FromStr>::from_str(&url)
        .unwrap()
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::migrate::Migrator::new(old)
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    for statement in [
        "INSERT INTO chats(id, kind, title, created_at, updated_at) VALUES (-5, 'group', 'Old', 1, 1)",
        "INSERT INTO senders(id, kind, display_name, created_at, updated_at) VALUES (3, 'user', 'Carol', 1, 1)",
        "INSERT INTO messages(chat_id, message_id, sender_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (-5, 1, 3, 100, 100, 1, 1, 'kept', 100, 0)",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let app = router(application(&store));
    let (status, body) = get(&app, "/api/v1/chats/-5/messages/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["text"], "kept");
    assert_eq!(body["sender"]["display_name"], "Carol");
    let (_, chat) = get(&app, "/api/v1/chats/-5").await;
    assert_eq!(chat["stats"]["message_count"], 1);
    store.close().await;

    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'messages_chat_%' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        indexes,
        [
            "messages_chat_all_order",
            "messages_chat_order",
            "messages_chat_sender",
            "messages_chat_stats"
        ]
    );
    pool.close().await;
}

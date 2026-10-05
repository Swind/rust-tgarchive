//! Search through the REST router and the CLI against real SQLite archives.

use std::{process::Command, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::DateTime;
use serde_json::Value;
use tgarchive::{
    application::{
        ArchiveWriter, IngestBatch, IngestRecord, MessageSource,
        services::{Application, ComponentState, ComponentStatus},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
    infrastructure::persistence::sqlite::SqliteStore,
    interface::rest::router,
};
use tower::ServiceExt;

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");
const BASE: i64 = 1_700_000_000;

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
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

async fn seed(store: &SqliteStore, texts: &[&str]) {
    let chat_id = ChatId::from_telegram(ChatKind::Group, 5).unwrap();
    let chat = Chat {
        id: chat_id,
        kind: ChatKind::Group,
        title: Some("測試群組".into()),
        username: None,
        tracked: false,
    };
    let records = texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let time = DateTime::from_timestamp(BASE + index as i64, 0).unwrap();
            IngestRecord {
                event: MessageEvent::Created(Message {
                    post_author: None,
                    forward: None,
                    id: MessageId::new(index as i64 + 1).unwrap(),
                    chat_id,
                    sender_id: None,
                    timestamp: time,
                    edited_at: None,
                    collected_at: time,
                    text: Some((*text).to_owned()),
                    reply_to: None,
                    attachments: vec![],
                }),
                source: MessageSource::History,
            }
        })
        .collect();
    store
        .write_batch(IngestBatch {
            chats: vec![chat],
            records,
            ..IngestBatch::default()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn rest_search_sorts_snippets_cursors_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let texts: Vec<String> = (0..9)
        .map(|n| format!("第{n}則 今天去台北喝咖啡，{}", "很好喝".repeat(n % 4)))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    seed(&store, &refs).await;
    let app = router(Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        None,
        ComponentStatus::new(ComponentState::Running, None),
    )));

    let (status, body) = get(&app, "/api/v1/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["search_index"]["state"], "ready");
    assert_eq!(body["search_index"]["version"], 1);
    assert_eq!(body["search_index"]["indexed"], 9);
    assert_eq!(body["search_index"]["total"], 9);

    let q = encode("台北咖啡");
    let (status, page) = get(&app, &format!("/api/v1/messages/search?q={q}&limit=100")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 9);
    assert!(
        page["items"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("台北")
    );

    // Relevance paging through the opaque cursor (passed back as `before`).
    for sort in ["", "&sort=relevance", "&sort=time"] {
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut uri = format!("/api/v1/messages/search?q={q}&limit=4{sort}");
            if let Some(cursor) = &cursor {
                uri.push_str(&format!("&before={cursor}"));
            }
            let (status, page) = get(&app, &uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {page}");
            seen.extend(
                page["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| m["id"].as_i64().unwrap()),
            );
            match page["next_cursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(seen.len(), 9, "sort {sort:?}: {seen:?}");
        assert_eq!(sorted.len(), 9, "sort {sort:?} duplicates: {seen:?}");
    }

    // time sort is newest first.
    let (_, page) = get(&app, &format!("/api/v1/messages/search?q={q}&sort=time")).await;
    assert_eq!(page["items"][0]["id"], 9);

    // Cursor misuse and invalid input are 400s.
    let (_, first) = get(
        &app,
        &format!("/api/v1/messages/search?q={q}&limit=2&sort=time"),
    )
    .await;
    let keyset = first["next_cursor"].as_str().unwrap();
    for uri in [
        format!("/api/v1/messages/search?q={q}&before={keyset}"),
        format!("/api/v1/messages/search?q={q}&after={keyset}"),
        format!("/api/v1/messages/search?q={q}&sort=bogus"),
        format!("/api/v1/messages/search?q={q}&before=garbage"),
        format!("/api/v1/messages/search?q={}", encode("！？")),
    ] {
        let (status, body) = get(&app, &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
    }
    let (_, relevance) = get(&app, &format!("/api/v1/messages/search?q={q}&limit=2")).await;
    let offset_cursor = relevance["next_cursor"].as_str().unwrap();
    let (status, _) = get(
        &app,
        &format!("/api/v1/messages/search?q={q}&sort=time&before={offset_cursor}"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "offset cursor is not a keyset cursor"
    );

    // Single character: LIKE scan, still ranked newest first with snippets.
    let (status, page) = get(&app, &format!("/api/v1/messages/search?q={}", encode("北"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 9);
    assert!(page["items"][0]["snippet"].as_str().unwrap().contains('北'));
}

fn run(url: &str, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("DATABASE_URL", url)
        .args(args)
        .output()
        .unwrap()
}

#[tokio::test]
async fn db_init_upgrades_an_old_archive_and_search_commands_report_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("old.db").display());
    let old = dir.path().join("old-migrations");
    std::fs::create_dir(&old).unwrap();
    for name in [
        "0001_archive.sql",
        "0002_telegram_account_deletions.sql",
        "0003_chat_tracking.sql",
        "0004_sync_rate_limited_state.sql",
        "0005_read_indexes.sql",
    ] {
        std::fs::copy(
            std::path::Path::new("migrations").join(name),
            old.join(name),
        )
        .unwrap();
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
        "INSERT INTO messages(chat_id, message_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (-5, 1, 100, 100, 1, 1, '今天去台北喝咖啡', 100, 0)",
        "INSERT INTO messages(chat_id, message_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (-5, 2, 101, 101, 1, 1, 'GitLab Runner', 101, 0)",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    // Query-only access before any rebuild: stale index, LIKE fallback still finds the text.
    // (Read-only opens do not migrate, so the 0005 schema is queried as-is.)
    let status = run(&url, &["--output", "json", "search", "status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(json["state"], "stale");
    let found = run(&url, &["--output", "json", "messages", "search", "台北喝"]);
    let page: Value = serde_json::from_slice(&found.stdout).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);

    let init = run(&url, &["db", "init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&init.stdout).trim(),
        "Initialized archive database."
    );
    let stderr = String::from_utf8_lossy(&init.stderr);
    assert!(stderr.contains("rebuilding"), "{stderr}");

    let status = run(&url, &["search", "status"]);
    let human = String::from_utf8_lossy(&status.stdout);
    assert!(human.contains("ready") && human.contains("2/2"), "{human}");
    let found = run(
        &url,
        &["--output", "json", "messages", "search", "台北咖啡"],
    );
    let page: Value = serde_json::from_slice(&found.stdout).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(
        page["items"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("台北")
    );

    // A second init is quiet; an explicit rebuild reports its result.
    let init = run(&url, &["db", "init"]);
    assert!(!String::from_utf8_lossy(&init.stderr).contains("rebuilding"));
    let rebuild = run(&url, &["--output", "json", "search", "rebuild-index"]);
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    let json: Value = serde_json::from_slice(&rebuild.stdout).unwrap();
    assert_eq!(
        (json["rebuilt"].clone(), json["indexed"].clone()),
        (true.into(), 2.into())
    );

    // relevance cursor flags are validated.
    let bad = run(&url, &["messages", "search", "台北", "--after", "x"]);
    assert!(!bad.status.success());
}

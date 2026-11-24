//! Search through the REST router and the CLI against real SQLite archives.

use std::{process::Command, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use chrono::DateTime;
use serde_json::{Value, json};
use tgarchive::{
    application::{
        ArchiveWriter, IngestBatch, IngestRecord, MessageSource,
        services::{Application, ComponentState, ComponentStatus},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
    infrastructure::persistence::{search_groups::SearchGroupStore, sqlite::SqliteStore},
    interface::rest::{router, router_with_groups},
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
    request(app, Method::GET, uri, None).await
}

async fn request(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap())),
        None => builder.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn seed(store: &SqliteStore, kind: ChatKind, raw_id: i64, texts: &[&str]) {
    let chat_id = ChatId::from_telegram(kind, raw_id).unwrap();
    let chat = Chat {
        id: chat_id,
        kind,
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
    seed(&store, ChatKind::Group, 5, &refs).await;
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

#[tokio::test]
async fn rest_search_excludes_multiple_chats_before_paging() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    for (kind, id) in [
        (ChatKind::Group, 5),
        (ChatKind::Private, 6),
        (ChatKind::Group, 7),
    ] {
        seed(&store, kind, id, &["coffee", "coffee"]).await;
    }
    let app = router(Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store,
        None,
        ComponentStatus::new(ComponentState::Running, None),
    )));
    let search = "/api/v1/messages/search?q=coffee";
    for sort in ["time", "relevance"] {
        let uri = format!("{search}&sort={sort}&limit=1&exclude_chat_ids=6,-5,6");
        let (status, page) = get(&app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["items"][0]["chat_id"], -7);
        let cursor = page["next_cursor"].as_str().unwrap();
        let (status, next) = get(&app, &format!("{uri}&before={cursor}")).await;
        assert_eq!(status, StatusCode::OK, "{next}");
        assert_eq!(next["items"][0]["chat_id"], -7);
        assert_ne!(next["items"][0]["id"], page["items"][0]["id"]);
        assert!(next["next_cursor"].is_null());
    }

    let max = ["6"; 100].join(",");
    let (status, _) = get(&app, &format!("{search}&exclude_chat_ids={max}")).await;
    assert_eq!(status, StatusCode::OK);
    for invalid in [
        "",
        "6,",
        "6,,-5",
        "bad",
        "0",
        "9223372036854775808",
        &format!("{max},6"),
    ] {
        let (status, body) = get(&app, &format!("{search}&exclude_chat_ids={invalid}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{invalid}: {body}");
        assert_eq!(body["error"]["code"], "malformed_request");
    }
}

#[tokio::test]
async fn rest_search_groups_crud_and_scoped_paging() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let channel = ChatId::from_telegram(ChatKind::Channel, 7).unwrap().get();
    for (kind, id) in [
        (ChatKind::Group, 5),
        (ChatKind::Private, 6),
        (ChatKind::Channel, 7),
    ] {
        seed(&store, kind, id, &["coffee 杯", "coffee 杯"]).await;
    }
    let application = Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store,
        None,
        ComponentStatus::new(ComponentState::Running, None),
    ));
    let collection = "/api/v1/search-groups";
    let search = "/api/v1/messages/search";
    // A missing settings store must not widen scoped searches or disable archive reads.
    let disabled = router(application.clone());
    assert_eq!(
        get(&disabled, collection).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        get(&disabled, &format!("{search}?q=coffee&group_id=1"))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        get(&disabled, &format!("{search}?q=coffee")).await.0,
        StatusCode::OK
    );

    let settings = SearchGroupStore::open(&dir.path().join("settings.db"))
        .await
        .unwrap();
    let app = router_with_groups(application, None, None, None, Some(settings));
    assert_eq!(get(&app, collection).await, (StatusCode::OK, json!([])));
    let (status, group) = request(
        &app,
        Method::POST,
        collection,
        Some(json!({"name": "  工作  ", "chat_ids": [-5, channel]})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    let id = group["id"].as_i64().unwrap();
    assert!(id > 0);
    assert_eq!(group["name"], "工作");
    assert_eq!(group["chat_ids"].as_array().unwrap().len(), 2);
    let item = format!("{collection}/{id}");
    assert_eq!(get(&app, &item).await, (StatusCode::OK, group.clone()));
    assert_eq!(
        get(&app, collection).await,
        (StatusCode::OK, json!([group]))
    );

    // Both FTS orders and the one-character LIKE fallback intersect exclusions before paging.
    let one_char = encode("杯");
    for (q, sort) in [
        ("coffee", "relevance"),
        ("coffee", "time"),
        (one_char.as_str(), "time"),
    ] {
        let uri = format!("{search}?q={q}&sort={sort}&group_id={id}&exclude_chat_ids=-5&limit=1");
        let (status, first) = get(&app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["items"][0]["chat_id"], channel);
        let next = first["next_cursor"].as_str().unwrap();
        let (status, second) = get(&app, &format!("{uri}&before={next}")).await;
        assert_eq!(status, StatusCode::OK, "{second}");
        assert_eq!(second["items"][0]["chat_id"], channel);
        assert_ne!(first["items"][0]["id"], second["items"][0]["id"]);
        assert!(second["next_cursor"].is_null());
    }

    for bad in [
        json!({"name": " ", "chat_ids": []}),
        json!({"name": "字".repeat(81), "chat_ids": []}),
        json!({"name": "test"}),
        json!({"name": "test", "chat_ids": [0]}),
        json!({"name": "test", "chat_ids": [-5, -5]}),
        json!({"name": "test", "chat_ids": [6, 99]}),
    ] {
        let (status, error) = request(&app, Method::PUT, &item, Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        assert_eq!(error["error"]["code"], "malformed_request");
    }
    assert_eq!(
        get(&app, &item).await.1,
        group,
        "failed edits must be atomic"
    );
    for ids in [vec![6], vec![]] {
        let body = json!({"name":"Renamed", "chat_ids":ids});
        let (status, changed) = request(&app, Method::PUT, &item, Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{changed}");
        assert_eq!(changed, json!({"id":id, "name":"Renamed", "chat_ids":ids}));
        let (status, page) = get(&app, &format!("{search}?q=coffee&group_id={id}")).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let hits = page["items"].as_array().unwrap();
        assert_eq!(hits.len(), 2 * ids.len());
        assert!(hits.iter().all(|hit| hit["chat_id"] == 6));
    }
    assert_eq!(
        request(&app, Method::DELETE, &item, None).await,
        (StatusCode::NO_CONTENT, Value::Null)
    );
    assert_eq!(get(&app, collection).await, (StatusCode::OK, json!([])));
    assert_eq!(get(&app, &item).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        request(&app, Method::DELETE, &item, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            Method::PUT,
            &item,
            Some(json!({"name":"gone", "chat_ids":[]}))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &format!("{search}?q=coffee&group_id={id}"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    for invalid in ["0", "-1", "abc"] {
        assert_eq!(
            get(&app, &format!("{search}?q=coffee&group_id={invalid}"))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
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

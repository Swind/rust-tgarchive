//! Bot flag (`senders.is_bot`), `exclude_bots` filters, observed name history and migration 0008.

use std::{path::Path, process::Command, sync::Arc};

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
    domain::{
        Chat, ChatId, ChatKind, Message, MessageEvent, MessageId, Sender, SenderId, SenderKind,
    },
    infrastructure::persistence::sqlite::SqliteStore,
    interface::rest::router,
};
use tower::ServiceExt;

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");
const BASE: i64 = 1_700_000_000;

fn group() -> ChatId {
    ChatId::from_telegram(ChatKind::Group, 10).unwrap()
}

fn user(raw: i64) -> SenderId {
    SenderId::from_telegram(SenderKind::User, raw).unwrap()
}

fn sender(raw: i64, name: Option<&str>, username: Option<&str>, bot: Option<bool>) -> Sender {
    Sender {
        id: user(raw),
        kind: SenderKind::User,
        display_name: name.map(str::to_owned),
        username: username.map(str::to_owned),
        is_bot: bot,
    }
}

fn msg(id: i64, at: i64, from: Option<i64>) -> IngestRecord {
    let time = DateTime::from_timestamp(BASE + at, 0).unwrap();
    IngestRecord {
        event: MessageEvent::Created(Message {
            id: MessageId::new(id).unwrap(),
            chat_id: group(),
            sender_id: from.map(user),
            timestamp: time,
            edited_at: None,
            collected_at: time,
            text: Some(format!("hello body {id}")),
            reply_to: None,
            attachments: vec![],
            post_author: None,
            forward: None,
        }),
        source: MessageSource::History,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    url: String,
    store: Arc<SqliteStore>,
    app: axum::Router,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let app = app_for(&store);
    store
        .write_batch(IngestBatch {
            chats: vec![Chat {
                id: group(),
                kind: ChatKind::Group,
                title: Some("Dev".into()),
                username: None,
                tracked: true,
            }],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    Fixture {
        dir,
        url,
        store,
        app,
    }
}

fn app_for(store: &Arc<SqliteStore>) -> axum::Router {
    router(Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        None,
        ComponentStatus::new(ComponentState::Running, None),
    )))
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

async fn write(f: &Fixture, at: i64, senders: Vec<Sender>) {
    // One record per call gives the batch its observation time (`collected_at` = BASE + at).
    let from = senders.first().map(|sender| sender.id.get());
    f.store
        .write_batch(IngestBatch {
            senders,
            records: vec![msg(at, at, from)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
}

async fn history(f: &Fixture, raw: i64) -> Vec<(Option<String>, Option<String>, i64, i64)> {
    let (status, body) = get(&f.app, &format!("/api/v1/senders/{raw}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["name_history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            let text = |key: &str| h[key].as_str().map(str::to_owned);
            let time = |key: &str| {
                DateTime::parse_from_rfc3339(h[key].as_str().unwrap())
                    .unwrap()
                    .timestamp()
                    - BASE
            };
            (
                text("display_name"),
                text("username"),
                time("first_seen_at"),
                time("last_seen_at"),
            )
        })
        .collect()
}

fn s(value: &str) -> Option<String> {
    Some(value.to_owned())
}

#[tokio::test]
async fn bot_flag_is_exposed_and_a_null_observation_never_clears_it() {
    let f = fixture().await;
    f.store
        .write_batch(IngestBatch {
            senders: vec![
                sender(1, Some("Helper"), Some("helper_bot"), Some(true)),
                sender(2, Some("Human"), None, Some(false)),
                sender(3, Some("Unknown"), None, None),
            ],
            records: vec![msg(1, 1, Some(1)), msg(2, 2, Some(2)), msg(3, 3, Some(3))],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    // A later observation without the flag keeps it; an explicit value overrides it.
    write(&f, 10, vec![sender(1, Some("Helper"), None, None)]).await;
    write(&f, 11, vec![sender(2, None, None, None)]).await;
    write(&f, 12, vec![sender(3, None, None, Some(true))]).await;

    let (_, detail) = get(&f.app, "/api/v1/senders/1").await;
    assert_eq!(detail["is_bot"], true);
    let (_, detail) = get(&f.app, "/api/v1/senders/2").await;
    assert_eq!(detail["is_bot"], false);
    let (_, detail) = get(&f.app, "/api/v1/senders/3").await;
    assert_eq!(detail["is_bot"], true);
    write(&f, 13, vec![sender(3, None, None, Some(false))]).await;
    let (_, detail) = get(&f.app, "/api/v1/senders/3").await;
    assert_eq!(detail["is_bot"], false);

    let (_, page) = get(&f.app, "/api/v1/senders?sort=name").await;
    let bots: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| (i["id"].as_i64().unwrap(), i["is_bot"].clone()))
        .collect();
    assert!(bots.contains(&(1, Value::Bool(true))));
    let (_, message) = get(&f.app, "/api/v1/chats/-10/messages/1").await;
    assert_eq!(message["sender"]["is_bot"], true);
    let (_, message) = get(&f.app, "/api/v1/chats/-10/messages/3").await;
    assert_eq!(message["sender"]["is_bot"], false);
}

#[tokio::test]
async fn senders_is_bot_filter_selects_bots_or_non_bots() {
    let f = fixture().await;
    f.store
        .write_batch(IngestBatch {
            senders: vec![
                sender(1, Some("Bot"), None, Some(true)),
                sender(2, Some("Human"), None, Some(false)),
                sender(3, Some("Unknown"), None, None),
            ],
            records: vec![msg(1, 1, Some(1)), msg(2, 2, Some(2)), msg(3, 3, Some(3))],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let ids = |page: &Value| -> Vec<i64> {
        let mut ids: Vec<i64> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_i64().unwrap())
            .collect();
        ids.sort();
        ids
    };
    let (_, page) = get(&f.app, "/api/v1/senders?is_bot=true").await;
    assert_eq!(ids(&page), [1]);
    let (_, page) = get(&f.app, "/api/v1/senders?is_bot=false").await;
    assert_eq!(ids(&page), [2, 3]);
    let (_, page) = get(&f.app, "/api/v1/senders").await;
    assert_eq!(ids(&page), [1, 2, 3]);
    let (_, page) = get(&f.app, "/api/v1/senders?is_bot=true&q=Human").await;
    assert!(ids(&page).is_empty());
}

#[tokio::test]
async fn exclude_bots_filters_lists_and_both_search_orders() {
    let f = fixture().await;
    f.store
        .write_batch(IngestBatch {
            senders: vec![
                sender(1, Some("Bot"), None, Some(true)),
                sender(2, Some("Human"), None, Some(false)),
                sender(3, Some("Unknown"), None, None),
            ],
            records: vec![
                msg(1, 1, Some(1)),
                msg(2, 2, Some(2)),
                msg(3, 3, Some(3)),
                msg(4, 4, None),
                msg(5, 5, Some(1)),
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let ids = |page: &Value| -> Vec<i64> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_i64().unwrap())
            .collect()
    };
    let (_, page) = get(&f.app, "/api/v1/messages").await;
    assert_eq!(ids(&page), [5, 4, 3, 2, 1]);
    for base in ["/api/v1/messages", "/api/v1/chats/-10/messages"] {
        let (_, page) = get(&f.app, &format!("{base}?exclude_bots=true")).await;
        assert_eq!(ids(&page), [4, 3, 2], "{base}");
        let (_, page) = get(&f.app, &format!("{base}?exclude_bots=false")).await;
        assert_eq!(ids(&page).len(), 5);
    }
    for sort in ["relevance", "time"] {
        let (_, page) = get(
            &f.app,
            &format!("/api/v1/messages/search?q=hello&sort={sort}"),
        )
        .await;
        assert_eq!(ids(&page).len(), 5, "{sort}");
        let (status, page) = get(
            &f.app,
            &format!("/api/v1/messages/search?q=hello&sort={sort}&exclude_bots=true"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let mut found = ids(&page);
        found.sort();
        assert_eq!(found, [2, 3, 4], "{sort}");
    }
}

#[tokio::test]
async fn name_history_appends_on_name_username_or_both_changes() {
    let f = fixture().await;
    write(&f, 10, vec![sender(1, Some("Ann"), Some("ann"), None)]).await;
    write(&f, 20, vec![sender(1, Some("Anna"), Some("ann"), None)]).await;
    write(&f, 30, vec![sender(1, Some("Anna"), Some("anna_x"), None)]).await;
    write(&f, 40, vec![sender(1, Some("Anya"), Some("anya"), None)]).await;
    assert_eq!(
        history(&f, 1).await,
        [
            (s("Anya"), s("anya"), 40, 40),
            (s("Anna"), s("anna_x"), 30, 30),
            (s("Anna"), s("ann"), 20, 20),
            (s("Ann"), s("ann"), 10, 10),
        ]
    );
}

#[tokio::test]
async fn name_history_ignores_repeats_and_null_fields_but_advances_last_seen() {
    let f = fixture().await;
    write(&f, 10, vec![sender(1, Some("Ann"), Some("ann"), None)]).await;
    write(&f, 20, vec![sender(1, Some("Ann"), Some("ann"), None)]).await;
    // NULL fields keep the stored value: not a change.
    write(&f, 30, vec![sender(1, None, Some("ann"), None)]).await;
    write(&f, 40, vec![sender(1, Some("Ann"), None, None)]).await;
    write(&f, 50, vec![sender(1, None, None, Some(true))]).await;
    assert_eq!(history(&f, 1).await, [(s("Ann"), s("ann"), 10, 50)]);

    // A sender without any name or username has no history; it starts with the first value.
    write(&f, 60, vec![sender(2, None, None, None)]).await;
    write(&f, 61, vec![sender(2, None, None, Some(false))]).await;
    let (status, detail) = get(&f.app, "/api/v1/senders/2").await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(detail["name_history"].as_array().unwrap().is_empty());
    write(&f, 70, vec![sender(2, None, Some("two"), None)]).await;
    write(&f, 80, vec![sender(2, None, None, None)]).await;
    assert_eq!(history(&f, 2).await, [(None, s("two"), 70, 80)]);
}

#[tokio::test]
async fn out_of_order_observation_neither_flip_flops_nor_moves_last_seen_back() {
    let f = fixture().await;
    write(&f, 100, vec![sender(1, Some("Old"), None, None)]).await;
    write(&f, 200, vec![sender(1, Some("New"), None, None)]).await;
    // A history sync that finished later but observed the sender with an earlier timestamp.
    write(&f, 150, vec![sender(1, Some("New"), None, None)]).await;
    assert_eq!(
        history(&f, 1).await,
        [(s("New"), None, 200, 200), (s("Old"), None, 100, 100)]
    );
}

#[tokio::test]
async fn senders_search_matches_old_names_and_reports_the_matched_history() {
    let f = fixture().await;
    f.store
        .write_batch(IngestBatch {
            senders: vec![
                sender(1, Some("王小明"), Some("xm"), None),
                sender(2, Some("Bob"), Some("bob"), None),
            ],
            records: vec![msg(1, 1, Some(1)), msg(2, 2, Some(2))],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    write(&f, 50, vec![sender(1, Some("小明同學"), Some("xm2"), None)]).await;

    let (_, page) = get(&f.app, "/api/v1/senders?q=%E7%8E%8B").await; // 王
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], 1);
    assert_eq!(items[0]["display_name"], "小明同學");
    assert_eq!(items[0]["matched_history"], true);
    assert_eq!(items[0]["matched_name"]["display_name"], "王小明");
    assert_eq!(items[0]["matched_name"]["username"], "xm");

    // Current names match normally, without the history flag.
    let (_, page) = get(&f.app, "/api/v1/senders?q=%E5%90%8C%E5%AD%B8").await; // 同學
    assert_eq!(page["items"][0]["matched_history"], false);
    assert!(page["items"][0]["matched_name"].is_null());
    // An exact old @username.
    let (_, page) = get(&f.app, "/api/v1/senders?q=%40xm").await;
    assert_eq!(page["items"][0]["id"], 1);
    assert_eq!(page["items"][0]["matched_history"], true);
    let (_, page) = get(&f.app, "/api/v1/senders").await;
    assert!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["matched_history"] == false)
    );
}

fn cli(url: &str, args: &[&str]) -> (bool, String) {
    let output = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("DATABASE_URL", url)
        .args(args)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[tokio::test]
async fn cli_shows_bot_history_and_excludes_bots() {
    let f = fixture().await;
    f.store
        .write_batch(IngestBatch {
            senders: vec![
                sender(1, Some("Ann"), Some("ann"), Some(false)),
                sender(2, Some("Helper"), Some("helper_bot"), Some(true)),
            ],
            records: vec![msg(1, 1, Some(1)), msg(2, 2, Some(2))],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    write(&f, 50, vec![sender(1, Some("Anna"), Some("ann"), None)]).await;
    f.store.close().await;
    let url = f.url.clone();
    let _keep = &f.dir;

    let (ok, out) = cli(&url, &["--output", "json", "senders", "get", "1"]);
    assert!(ok);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["is_bot"], false);
    assert_eq!(json["name_history"].as_array().unwrap().len(), 2);
    assert_eq!(json["name_history"][0]["display_name"], "Anna");
    let (ok, out) = cli(&url, &["senders", "get", "1"]);
    assert!(
        ok && out.contains("name history") && out.contains("Ann\t@ann"),
        "{out}"
    );

    let (ok, out) = cli(&url, &["senders", "search", "Ann"]);
    assert!(ok && !out.contains("matched old name"), "{out}");
    let (ok, out) = cli(&url, &["--output", "json", "senders", "search", "helper"]);
    assert!(ok);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json[0]["is_bot"], true);
    let (ok, out) = cli(&url, &["senders", "search", "helper"]);
    assert!(ok && out.contains("\tbot"), "{out}");
    let (ok, out) = cli(&url, &["senders", "search", "--is-bot", "true", "e"]);
    assert!(
        ok && out.contains("Helper") && !out.contains("Anna"),
        "{out}"
    );

    let ids = |args: &[&str]| -> Vec<i64> {
        let (ok, out) = cli(&url, args);
        assert!(ok, "{out}");
        let json: Value = serde_json::from_str(&out).unwrap();
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_i64().unwrap())
            .collect()
    };
    assert_eq!(ids(&["--output", "json", "messages", "list"]).len(), 3);
    assert_eq!(
        ids(&["--output", "json", "messages", "list", "--exclude-bots"]),
        [50, 1]
    );
    assert_eq!(
        ids(&[
            "--output",
            "json",
            "messages",
            "search",
            "hello",
            "--exclude-bots"
        ])
        .len(),
        2
    );
}

#[tokio::test]
async fn cli_marks_old_name_matches() {
    let f = fixture().await;
    write(&f, 1, vec![sender(1, Some("Oldie"), None, None)]).await;
    write(&f, 2, vec![sender(1, Some("Newbie"), None, None)]).await;
    f.store.close().await;
    let _keep = &f.dir;
    let (ok, out) = cli(&f.url, &["senders", "search", "oldie"]);
    assert!(ok && out.contains("[matched old name: Oldie]"), "{out}");
}

#[tokio::test]
async fn migration_0008_backfills_history_from_a_0007_database() {
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
        "0006_search_index.sql",
        "0007_sender_metadata.sql",
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
        "INSERT INTO chats(id, kind, title, created_at, updated_at) VALUES (-10, 'group', 'Dev', 1, 1)",
        "INSERT INTO senders(id, kind, display_name, username, created_at, updated_at) VALUES (1, 'user', 'Carol', 'carol', 100, 200)",
        "INSERT INTO senders(id, kind, display_name, created_at, updated_at) VALUES (2, 'user', 'Dave', 300, 400)",
        "INSERT INTO senders(id, kind, username, created_at, updated_at) VALUES (3, 'user', 'eve', 500, 600)",
        "INSERT INTO senders(id, kind, created_at, updated_at) VALUES (4, 'user', 700, 800)",
        "INSERT INTO messages(chat_id, message_id, sender_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (-10, 1, 1, 100, 100, 1, 1, 'kept', 100, 0)",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let app = app_for(&store);
    let (_, detail) = get(&app, "/api/v1/senders/1").await;
    assert!(detail["is_bot"].is_null());
    let history = detail["name_history"].as_array().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["display_name"], "Carol");
    assert_eq!(history[0]["username"], "carol");
    assert_eq!(
        DateTime::parse_from_rfc3339(history[0]["first_seen_at"].as_str().unwrap())
            .unwrap()
            .timestamp(),
        100
    );
    assert_eq!(
        DateTime::parse_from_rfc3339(history[0]["last_seen_at"].as_str().unwrap())
            .unwrap()
            .timestamp(),
        200
    );
    let (_, page) = get(&app, "/api/v1/messages").await;
    assert_eq!(page["items"][0]["text"], "kept");
    assert!(page["items"][0]["sender"]["is_bot"].is_null());
    store.close().await;

    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let rows: Vec<(i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT sender_id, display_name, username FROM sender_name_history ORDER BY sender_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            (1, s("Carol"), s("carol")),
            (2, s("Dave"), None),
            (3, None, s("eve")),
        ]
    );
    let index: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='sender_name_history_sender'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(index, 1);
    pool.close().await;
}

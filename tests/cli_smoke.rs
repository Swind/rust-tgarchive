use std::{process::Command, time::Duration};

use chrono::{DateTime, Utc};
use serde_json::Value;
use tempfile::TempDir;
use tgarchive::{
    application::{ArchiveWriter, IngestBatch, IngestRecord, MessageSource},
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
    infrastructure::persistence::sqlite::SqliteStore,
};

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");

fn database_url(directory: &TempDir) -> String {
    format!("sqlite://{}", directory.path().join("archive.db").display())
}

fn invoke(url: &str, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("DATABASE_URL", url)
        .args(args)
        .output()
        .unwrap()
}

fn fixture() -> (Chat, Message) {
    let chat = Chat {
        id: ChatId::from_telegram(ChatKind::Channel, 25).unwrap(),
        kind: ChatKind::Channel,
        title: Some("fixture channel".into()),
        username: Some("fixture".into()),
        tracked: false,
    };
    let timestamp = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    let message = Message {
        post_author: None,
        forward: None,
        id: MessageId::new(7).unwrap(),
        chat_id: chat.id,
        sender_id: None,
        timestamp,
        edited_at: None,
        collected_at: timestamp + Duration::from_secs(2),
        text: Some("hello from fixture".into()),
        reply_to: None,
        attachments: vec![],
    };
    (chat, message)
}

async fn seed(url: &str) {
    let (chat, message) = fixture();
    let store = SqliteStore::connect(url).await.unwrap();
    store
        .write_batch(IngestBatch {
            chats: vec![chat],
            records: vec![
                IngestRecord {
                    event: MessageEvent::Created(message.clone()),
                    source: MessageSource::Realtime,
                },
                IngestRecord {
                    event: MessageEvent::Created(Message {
                        id: MessageId::new(6).unwrap(),
                        timestamp: message.timestamp - chrono::Duration::seconds(1),
                        collected_at: message.collected_at,
                        text: Some("older message".into()),
                        ..message
                    }),
                    source: MessageSource::Realtime,
                },
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    store.close().await;
}

#[test]
fn help_and_bad_arguments_need_no_database_or_telegram_credentials() {
    let help = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("messages"));

    let bad_limit = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .args(["messages", "list", "--limit", "0"])
        .output()
        .unwrap();
    assert!(!bad_limit.status.success());
    assert!(!String::from_utf8_lossy(&bad_limit.stderr).contains("DATABASE_URL"));

    let bad_cursor = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .args(["messages", "list", "--before", "not-a-cursor"])
        .output()
        .unwrap();
    assert!(!bad_cursor.status.success());
    assert!(String::from_utf8_lossy(&bad_cursor.stderr).contains("cursor is malformed"));

    let openapi = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .arg("openapi")
        .output()
        .unwrap();
    assert!(openapi.status.success());
    let document: Value = serde_json::from_slice(&openapi.stdout).unwrap();
    assert!(document["paths"].get("/api/v1/messages").is_some());

    let yaml = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .args(["openapi", "--format", "yaml"])
        .output()
        .unwrap();
    assert!(yaml.status.success());
    let yaml: Value = serde_norway::from_slice(&yaml.stdout).unwrap();
    assert!(yaml["paths"].get("/api/v1/messages").is_some());

    let rejected_bind = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .args(["serve", "--bind", "0.0.0.0:8080"])
        .output()
        .unwrap();
    assert!(!rejected_bind.status.success());
    assert!(String::from_utf8_lossy(&rejected_bind.stderr).contains("loopback"));
}

#[tokio::test]
async fn db_init_and_readonly_json_queries_work_without_telegram_environment() {
    let directory = tempfile::tempdir().unwrap();
    let url = database_url(&directory);
    let initialized = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .args(["--output", "json", "db", "init", "--database-url", &url])
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&initialized.stdout).unwrap(),
        serde_json::json!({"initialized":true})
    );
    assert!(initialized.stderr.is_empty());

    seed(&url).await;
    let (chat, message) = fixture();
    let chat_id = chat.id.get().to_string();

    let got_chat = invoke(&url, &["chats", "get", &chat_id, "--output", "json"]);
    assert!(
        got_chat.status.success(),
        "{}",
        String::from_utf8_lossy(&got_chat.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&got_chat.stdout).unwrap()["id"],
        chat.id.get()
    );

    let chats = invoke(&url, &["--output", "json", "chats", "list"]);
    assert!(
        chats.status.success(),
        "{}",
        String::from_utf8_lossy(&chats.stderr)
    );
    assert!(chats.stderr.is_empty());
    let chats: Value = serde_json::from_slice(&chats.stdout).unwrap();
    assert_eq!(chats[0]["id"], chat.id.get());

    let listed = invoke(
        &url,
        &[
            "messages",
            "list",
            "--chat-id",
            &chat_id,
            "--output",
            "json",
        ],
    );
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    assert!(listed.stderr.is_empty());
    let listed: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["items"][0]["id"], message.id.get());
    assert_eq!(listed["items"][0]["chat_id"], chat.id.get());
    assert_eq!(listed["has_more"], false);
    assert_eq!(listed["items"].as_array().unwrap().len(), 2);

    let human_page = invoke(
        &url,
        &["messages", "list", "--chat-id", &chat_id, "--limit", "1"],
    );
    assert!(
        human_page.status.success(),
        "{}",
        String::from_utf8_lossy(&human_page.stderr)
    );
    let human_page = String::from_utf8(human_page.stdout).unwrap();
    assert!(human_page.contains("has_more\ttrue"));
    let token = human_page
        .lines()
        .find_map(|line| line.strip_prefix("next_cursor\t"))
        .unwrap();
    assert_eq!(
        tgarchive::interface::cursor::decode(token)
            .unwrap()
            .message_id
            .get(),
        message.id.get()
    );

    let searched = invoke(
        &url,
        &[
            "--output",
            "json",
            "messages",
            "search",
            "hello",
            "--chat-id",
            &chat_id,
        ],
    );
    assert!(
        searched.status.success(),
        "{}",
        String::from_utf8_lossy(&searched.stderr)
    );
    assert!(searched.stderr.is_empty());
    let searched: Value = serde_json::from_slice(&searched.stdout).unwrap();
    assert_eq!(searched["items"][0]["text"], "hello from fixture");

    let get_args = ["messages", "get", chat_id.as_str(), "7", "--output", "json"];
    let got = invoke(&url, &get_args);
    assert!(
        got.status.success(),
        "{}",
        String::from_utf8_lossy(&got.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&got.stdout).unwrap()["text"],
        "hello from fixture"
    );

    let status = invoke(&url, &["--output", "json", "status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["collector"]["state"], "disabled");
    assert!(status["sync_jobs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn readonly_query_does_not_initialize_an_existing_empty_database() {
    let directory = tempfile::tempdir().unwrap();
    let url = database_url(&directory);
    std::fs::File::create(directory.path().join("archive.db")).unwrap();

    let output = invoke(&url, &["chats", "list"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no such table: chats"));

    let missing_url = format!("sqlite://{}", directory.path().join("missing.db").display());
    let missing = invoke(&missing_url, &["chats", "list"]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("create it with `tgarchive db init`")
    );
}

#[test]
fn committed_openapi_yml_matches_generated_output() {
    let output = std::process::Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .args(["openapi", "--format", "yaml"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let committed = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.yml"))
        .expect("openapi.yml must be committed");
    assert_eq!(
        committed,
        String::from_utf8(output.stdout).unwrap(),
        "openapi.yml is stale; regenerate: cargo run -q -- openapi --format yaml > openapi.yml"
    );
}

#[test]
fn misconfiguration_fails_at_startup_with_a_fix_hint() {
    let dir = tempfile::tempdir().unwrap();
    let missing_db = format!("sqlite://{}/nope/a.db", dir.path().display());
    let out = std::process::Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("DATABASE_URL", &missing_db)
        .args(["serve", "--query-only"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("does not exist"));

    let out = std::process::Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("TELEGRAM_API_ID", "1")
        .env("TELEGRAM_API_HASH", "SECRET-HASH-VALUE")
        .env("TELEGRAM_SESSION_FILE", dir.path().join("no/dir/s.session"))
        .env(
            "DATABASE_URL",
            format!("sqlite://{}/a.db", dir.path().display()),
        )
        .args(["serve"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("TELEGRAM_SESSION_FILE"), "{stderr}");
    assert!(!stderr.contains("SECRET-HASH-VALUE"));

    let out = std::process::Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env(
            "DATABASE_URL",
            format!("sqlite://{}/a.db", dir.path().display()),
        )
        .args(["serve", "--query-only", "--bind", "0.0.0.0:0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("loopback"));
}

#[tokio::test]
async fn include_deleted_flag_and_sender_names_work_in_the_cli() {
    let directory = tempfile::tempdir().unwrap();
    let url = database_url(&directory);
    seed(&url).await;
    let (chat, message) = fixture();
    let sender =
        tgarchive::domain::SenderId::from_telegram(tgarchive::domain::SenderKind::User, 9).unwrap();
    let store = SqliteStore::connect(&url).await.unwrap();
    store
        .write_batch(IngestBatch {
            senders: vec![tgarchive::domain::Sender {
                id: sender,
                kind: tgarchive::domain::SenderKind::User,
                display_name: Some("Dana".into()),
                username: None,
            }],
            records: vec![
                IngestRecord {
                    event: MessageEvent::Created(Message {
                        sender_id: Some(sender),
                        ..message.clone()
                    }),
                    source: MessageSource::Realtime,
                },
                IngestRecord {
                    event: MessageEvent::Deleted {
                        chat_id: chat.id,
                        message_id: MessageId::new(6).unwrap(),
                        deleted_at: message.timestamp + chrono::Duration::seconds(5),
                    },
                    source: MessageSource::Realtime,
                },
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    store.close().await;
    let chat_id = chat.id.get().to_string();

    let hidden = invoke(&url, &["messages", "list", "--chat-id", &chat_id]);
    let hidden = String::from_utf8_lossy(&hidden.stdout).into_owned();
    assert!(hidden.contains("Dana\thello from fixture"), "{hidden}");
    assert!(!hidden.contains("older message"), "{hidden}");

    let shown = invoke(
        &url,
        &[
            "messages",
            "list",
            "--chat-id",
            &chat_id,
            "--include-deleted",
        ],
    );
    assert!(String::from_utf8_lossy(&shown.stdout).contains("[deleted] older message"));
    let search = invoke(&url, &["messages", "search", "older", "--include-deleted"]);
    assert!(String::from_utf8_lossy(&search.stdout).contains("[deleted] older message"));
    let search_hidden = invoke(&url, &["messages", "search", "older"]);
    assert!(!String::from_utf8_lossy(&search_hidden.stdout).contains("older message"));

    let missing = invoke(&url, &["messages", "get", &chat_id, "6"]);
    assert!(!missing.status.success());
    let got = invoke(
        &url,
        &[
            "--output",
            "json",
            "messages",
            "get",
            &chat_id,
            "6",
            "--include-deleted",
        ],
    );
    assert!(
        got.status.success(),
        "{}",
        String::from_utf8_lossy(&got.stderr)
    );
    let json: Value = serde_json::from_slice(&got.stdout).unwrap();
    assert_eq!(json["text"], "older message");
    assert!(json["deleted_at"].is_string());
}

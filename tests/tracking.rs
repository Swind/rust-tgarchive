//! Opt-in collection: nothing is collected unless the chat is explicitly tracked.

use std::{
    collections::HashSet,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use tempfile::TempDir;
use tgarchive::{
    application::{
        AccountDeletion, ApplicationError, ArchiveWriter, ChatCheckpoint, ChatRepository,
        HistoryBoundary, HistoryPage, IngestBatch, IngestRecord, ListMessagesQuery, MessageFilters,
        MessageSource, PageSize, SyncChatProgress, SyncJob, SyncJobState, SyncRepository,
        SyncScope, TelegramError, TelegramGateway, TimeRange, TrackingScope, ingestion_worker,
        realtime::{
            RealtimeSource, RealtimeSourceError, ReconnectBackoff, restrict_to_tracked,
            supervise_realtime,
        },
        services::{Application, ComponentStatus},
        sync::{CancellationToken, SyncCoordinator, SyncEngine},
    },
    domain::{
        Chat, ChatId, ChatKind, Message, MessageEvent, MessageId, Sender, SenderId, SenderKind,
    },
    infrastructure::persistence::sqlite::SqliteStore,
    interface::rest::router_with_sync,
};
use tower::ServiceExt;

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");

fn private() -> ChatId {
    ChatId::from_telegram(ChatKind::Private, 5).unwrap()
}
fn group() -> ChatId {
    ChatId::from_telegram(ChatKind::Group, 6).unwrap()
}
fn channel() -> ChatId {
    ChatId::from_telegram(ChatKind::Channel, 7).unwrap()
}
fn chat(id: ChatId, kind: ChatKind) -> Chat {
    Chat {
        id,
        kind,
        title: Some(format!("chat {}", id.get())),
        username: None,
        tracked: false,
    }
}
fn all_chats() -> Vec<Chat> {
    vec![
        chat(private(), ChatKind::Private),
        chat(group(), ChatKind::Group),
        chat(channel(), ChatKind::Channel),
    ]
}
fn message(chat_id: ChatId, id: i64) -> Message {
    let timestamp = DateTime::<Utc>::from_timestamp(1_700_000_000 + id, 0).unwrap();
    Message {
        post_author: None,
        forward: None,
        id: MessageId::new(id).unwrap(),
        chat_id,
        sender_id: Some(SenderId::from_telegram(SenderKind::User, 5).unwrap()),
        timestamp,
        edited_at: None,
        collected_at: timestamp,
        text: Some(format!("m{id}")),
        reply_to: None,
        attachments: vec![],
    }
}
fn created(chat_id: ChatId, id: i64) -> IngestRecord {
    IngestRecord {
        event: MessageEvent::Created(message(chat_id, id)),
        source: MessageSource::Realtime,
    }
}
fn user() -> Sender {
    Sender {
        id: SenderId::from_telegram(SenderKind::User, 5).unwrap(),
        kind: SenderKind::User,
        display_name: Some("u".into()),
        username: None,
    }
}

fn url(directory: &TempDir) -> String {
    format!("sqlite://{}", directory.path().join("archive.db").display())
}

async fn store_with_chats() -> (TempDir, String, Arc<SqliteStore>) {
    let directory = tempfile::tempdir().unwrap();
    let url = url(&directory);
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    store.save_refresh(all_chats()).await.unwrap();
    (directory, url, store)
}

fn application(store: &Arc<SqliteStore>) -> Application {
    Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        None,
        ComponentStatus::disabled(),
    )
}

async fn tracked_ids(store: &SqliteStore) -> HashSet<i64> {
    store
        .tracked_chat_ids()
        .await
        .unwrap()
        .into_iter()
        .map(ChatId::get)
        .collect()
}

#[tokio::test]
async fn migration_adds_untracked_default_and_keeps_existing_rows() {
    let directory = tempfile::tempdir().unwrap();
    let url = url(&directory);
    // Apply only the first two migrations, insert a pre-existing chat with data, then upgrade.
    let old = directory.path().join("old-migrations");
    std::fs::create_dir(&old).unwrap();
    for name in ["0001_archive.sql", "0002_telegram_account_deletions.sql"] {
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
    sqlx::query("INSERT INTO chats(id, kind, created_at, updated_at) VALUES (42, 'private', 1, 1)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let store = SqliteStore::connect(&url).await.unwrap();
    let existing = ChatRepository::get(&store, ChatId::from_marked(42).unwrap())
        .await
        .unwrap()
        .expect("existing row survives the migration");
    assert!(!existing.tracked);
    // Rows created later (metadata refresh, or implicitly by message writes) are untracked too.
    store.save_refresh(all_chats()).await.unwrap();
    store
        .write_batch(IngestBatch {
            records: vec![created(ChatId::from_marked(43).unwrap(), 1)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    assert!(
        ChatRepository::list(&store)
            .await
            .unwrap()
            .iter()
            .all(|chat| !chat.tracked)
    );
    assert!(tracked_ids(&store).await.is_empty());
}

#[tokio::test]
async fn track_and_untrack_are_idempotent_keep_messages_and_survive_refresh() {
    let (_directory, _url, store) = store_with_chats().await;
    let app = application(&store);
    store
        .write_batch(IngestBatch {
            records: vec![created(private(), 1), created(private(), 2)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();

    assert!(app.track_chat(private()).await.unwrap().tracked);
    assert!(app.track_chat(private()).await.unwrap().tracked);
    assert_eq!(app.list_chats(true).await.unwrap().len(), 1);
    assert_eq!(app.list_chats(false).await.unwrap().len(), 3);

    // Metadata refresh (with Telegram-sourced `tracked: false`) never changes the flag.
    store.save_refresh(all_chats()).await.unwrap();
    assert!(app.get_chat(private()).await.unwrap().tracked);

    assert!(!app.untrack_chat(private()).await.unwrap().tracked);
    assert!(!app.untrack_chat(private()).await.unwrap().tracked);
    assert!(app.list_chats(true).await.unwrap().is_empty());
    store.save_refresh(all_chats()).await.unwrap();
    assert!(!app.get_chat(private()).await.unwrap().tracked);

    let page = app
        .list_messages(ListMessagesQuery {
            filters: MessageFilters {
                chat_id: Some(private()),
                sender_id: None,
                post_author: None,
                time_range: TimeRange::new(None, None).unwrap(),
                include_deleted: false,
            },
            before: None,
            after: None,
            page_size: PageSize::DEFAULT,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2, "untracking keeps stored messages");

    let unknown = ChatId::from_telegram(ChatKind::Private, 999).unwrap();
    assert!(matches!(
        app.track_chat(unknown).await,
        Err(ApplicationError::NotFound)
    ));
    assert!(matches!(
        app.untrack_chat(unknown).await,
        Err(ApplicationError::NotFound)
    ));
}

#[tokio::test]
async fn realtime_filter_drops_untracked_chats_and_noise_but_keeps_tracked() {
    let (_directory, _url, store) = store_with_chats().await;
    let app = application(&store);
    app.track_chat(private()).await.unwrap();
    store
        .write_batch(IngestBatch {
            records: vec![created(private(), 70)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let now = Utc::now();
    let batch = || IngestBatch {
        chats: all_chats(),
        senders: vec![user()],
        records: vec![
            created(private(), 1),
            created(group(), 2),
            IngestRecord {
                event: MessageEvent::Deleted {
                    chat_id: channel(),
                    message_id: MessageId::new(3).unwrap(),
                    deleted_at: now,
                },
                source: MessageSource::Realtime,
            },
        ],
        account_deletions: vec![
            AccountDeletion {
                message_id: MessageId::new(70).unwrap(),
                deleted_at: now,
            },
            AccountDeletion {
                message_id: MessageId::new(71).unwrap(),
                deleted_at: now,
            },
        ],
        ..IngestBatch::default()
    };

    let kept = restrict_to_tracked(store.as_ref(), batch()).await.unwrap();
    assert_eq!(
        kept.chats.iter().map(|c| c.id).collect::<Vec<_>>(),
        [private()]
    );
    assert_eq!(kept.records.len(), 1);
    assert_eq!(kept.senders.len(), 1);
    // Only the deletion of an already archived tracked-chat message survives: no tombstone noise.
    assert_eq!(
        kept.account_deletions
            .iter()
            .map(|d| d.message_id.get())
            .collect::<Vec<_>>(),
        [70]
    );

    // Tracking changes apply to the very next batch without restarting anything.
    app.untrack_chat(private()).await.unwrap();
    let kept = restrict_to_tracked(store.as_ref(), batch()).await.unwrap();
    assert!(kept.chats.is_empty() && kept.records.is_empty() && kept.senders.is_empty());
    assert!(kept.account_deletions.is_empty());
    app.track_chat(channel()).await.unwrap();
    let kept = restrict_to_tracked(store.as_ref(), batch()).await.unwrap();
    assert_eq!(
        kept.records.len(),
        1,
        "channel deletion of a tracked channel"
    );
    assert_eq!(kept.chats.len(), 1);
    assert!(
        kept.senders.is_empty(),
        "sender of dropped messages is not upserted"
    );
}

#[derive(Default)]
struct Gateway {
    fetched: Mutex<Vec<ChatId>>,
}

#[async_trait]
impl TelegramGateway for Gateway {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        Ok(all_chats())
    }
    async fn fetch_history(
        &self,
        chat_id: ChatId,
        _: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        self.fetched.lock().unwrap().push(chat_id);
        Ok(HistoryPage {
            chats: vec![],
            senders: vec![],
            records: vec![],
            next_before_message_id: None,
            next_after_message_id: None,
            exhausted: true,
        })
    }
}

struct Runtime {
    engine: Arc<SyncEngine>,
    coordinator: Arc<SyncCoordinator>,
    gateway: Arc<Gateway>,
    store: Arc<SqliteStore>,
    _directory: TempDir,
    url: String,
}

async fn runtime() -> Runtime {
    let (directory, url, store) = store_with_chats().await;
    let gateway = Arc::new(Gateway::default());
    let (sink, _writer) = ingestion_worker::spawn(store.clone(), 8);
    let engine = Arc::new(SyncEngine::new(
        gateway.clone(),
        store.clone(),
        store.clone(),
        sink,
        PageSize::DEFAULT,
    ));
    let coordinator = SyncCoordinator::spawn(engine.clone(), store.clone(), 4);
    Runtime {
        engine,
        coordinator,
        gateway,
        store,
        _directory: directory,
        url,
    }
}

#[tokio::test]
async fn sync_chat_rejects_untracked_and_sync_all_covers_only_tracked() {
    let rt = runtime().await;
    assert!(matches!(
        rt.coordinator.submit(SyncScope::Chat(private())).await,
        Err(ApplicationError::NotTracked)
    ));
    let unknown = ChatId::from_telegram(ChatKind::Private, 999).unwrap();
    assert!(matches!(
        rt.coordinator.submit(SyncScope::Chat(unknown)).await,
        Err(ApplicationError::NotFound)
    ));

    // Zero tracked chats: a clear, successful no-op.
    let job = rt.coordinator.submit(SyncScope::All).await.unwrap();
    let done = rt.coordinator.wait_job(&job.id).await.unwrap();
    assert_eq!(done.state, SyncJobState::Succeeded);
    assert!(rt.gateway.fetched.lock().unwrap().is_empty());

    application(&rt.store).track_chat(group()).await.unwrap();
    let job = rt.coordinator.submit(SyncScope::All).await.unwrap();
    let done = rt.coordinator.wait_job(&job.id).await.unwrap();
    assert_eq!(done.state, SyncJobState::Succeeded);
    assert_eq!(*rt.gateway.fetched.lock().unwrap(), [group()]);

    let job = rt
        .coordinator
        .submit(SyncScope::Chat(group()))
        .await
        .unwrap();
    rt.coordinator.wait_job(&job.id).await.unwrap();
    rt.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn catch_up_covers_only_tracked_chats() {
    let rt = runtime().await;
    application(&rt.store).track_chat(private()).await.unwrap();
    let summary = rt
        .engine
        .catch_up_all(&CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(summary.caught_up_chats, 1);
    assert_eq!(*rt.gateway.fetched.lock().unwrap(), [private()]);
    rt.coordinator.shutdown().await.unwrap();
}

struct IdleSource;

#[async_trait]
impl RealtimeSource for IdleSource {
    async fn run_once(&self, cancel: CancellationToken) -> Result<(), RealtimeSourceError> {
        cancel.cancelled().await;
        Ok(())
    }
}

#[tokio::test]
async fn chat_tracked_while_serving_gets_a_catch_up_baseline_without_restart() {
    let rt = runtime().await;
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let (engine, cancel) = (rt.engine.clone(), cancel.clone());
        async move {
            supervise_realtime(
                &IdleSource,
                &engine,
                ReconnectBackoff {
                    initial: Duration::from_secs(1),
                    max: Duration::from_secs(1),
                    healthy_after: Duration::from_secs(3600),
                    baseline_poll: Duration::from_millis(20),
                },
                cancel,
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        rt.gateway.fetched.lock().unwrap().is_empty(),
        "nothing tracked yet"
    );

    application(&rt.store).track_chat(private()).await.unwrap();
    for _ in 0..100 {
        let baseline: Option<ChatCheckpoint> =
            SyncRepository::get_checkpoint(rt.store.as_ref(), private())
                .await
                .unwrap();
        if baseline.is_some_and(|c| c.catchup_after_id.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let baseline = SyncRepository::get_checkpoint(rt.store.as_ref(), private())
        .await
        .unwrap()
        .expect("checkpoint created");
    assert!(baseline.catchup_after_id.is_some());
    assert_eq!(*rt.gateway.fetched.lock().unwrap(), [private()]);

    cancel.cancel();
    task.await.unwrap().unwrap();
    rt.coordinator.shutdown().await.unwrap();
}

async fn send(service: &axum::Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn rest_tracking_routes_filter_and_sync_rejection() {
    let rt = runtime().await;
    let service = router_with_sync(
        Arc::new(application(&rt.store)),
        Some(rt.coordinator.clone()),
    );
    let tracking = format!("/api/v1/chats/{}/tracking", private().get());

    let (status, body) = send(&service, "GET", "/api/v1/chats?tracked=true").await;
    assert_eq!(
        (status, body.as_array().unwrap().len()),
        (StatusCode::OK, 0)
    );

    let (status, body) = send(
        &service,
        "POST",
        &format!("/api/v1/chats/{}/sync", private().get()),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "chat_not_tracked");

    for _ in 0..2 {
        let (status, body) = send(&service, "PUT", &tracking).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tracked"], true);
        assert_eq!(body["id"], private().get());
    }
    let (_, body) = send(&service, "GET", "/api/v1/chats?tracked=true").await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    let (_, body) = send(&service, "GET", "/api/v1/chats").await;
    assert_eq!(body.as_array().unwrap().len(), 3);
    let (_, body) = send(
        &service,
        "GET",
        &format!("/api/v1/chats/{}", private().get()),
    )
    .await;
    assert_eq!(body["tracked"], true);

    let (status, job) = send(
        &service,
        "POST",
        &format!("/api/v1/chats/{}/sync", private().get()),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    rt.coordinator
        .wait_job(job["id"].as_str().unwrap())
        .await
        .unwrap();

    for _ in 0..2 {
        let (status, body) = send(&service, "DELETE", &tracking).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["tracked"], false);
    }

    let (status, body) = send(&service, "PUT", "/api/v1/chats/999/tracking").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(body["error"]["request_id"].is_string());
    let (status, _) = send(&service, "DELETE", "/api/v1/chats/999/tracking").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&service, "PUT", "/api/v1/chats/0/tracking").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(&service, "GET", "/api/v1/chats?tracked=maybe").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, spec) = send(&service, "GET", "/openapi.json").await;
    let path = &spec["paths"]["/api/v1/chats/{chat_id}/tracking"];
    assert!(path["put"].is_object() && path["delete"].is_object());
    assert!(spec["components"]["schemas"]["ChatDto"]["properties"]["tracked"].is_object());
    let _ = &rt.url;
    rt.coordinator.shutdown().await.unwrap();
}

fn cli(url: &str, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("DATABASE_URL", url)
        .args(args)
        .output()
        .unwrap()
}

#[tokio::test]
async fn cli_track_untrack_list_get_and_sync_rejection() {
    let (_directory, url, store) = store_with_chats().await;
    store
        .write_batch(IngestBatch {
            records: vec![created(private(), 1)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    store.close().await;
    let id = private().get().to_string();

    let out = cli(&url, &["chats", "list"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("untracked"));
    let out = cli(&url, &["--output", "json", "chats", "list", "--tracked"]);
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap(),
        serde_json::json!([])
    );

    for _ in 0..2 {
        let out = cli(&url, &["chats", "track", &id]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("Tracking chat"));
    }
    let out = cli(&url, &["--output", "json", "chats", "get", &id]);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["tracked"],
        true
    );
    let out = cli(&url, &["--output", "json", "chats", "list", "--tracked"]);
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let out = cli(&url, &["chats", "list", "--tracked"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("\ttracked"));

    for _ in 0..2 {
        let out = cli(&url, &["--output", "json", "chats", "untrack", &id]);
        assert!(out.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap()["tracked"],
            false
        );
    }
    // Stored messages survive untracking.
    let out = cli(
        &url,
        &["--output", "json", "messages", "list", "--chat-id", &id],
    );
    let page: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);

    for sub in ["track", "untrack"] {
        let out = cli(&url, &["chats", sub, "999"]);
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("not found") && stderr.contains("chats refresh"),
            "{stderr}"
        );
    }

    // No Telegram credentials are present: the rejection happens before Telegram is contacted.
    let out = cli(&url, &["sync", "chat", &id]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("track"));
    let out = cli(&url, &["sync", "all"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("No tracked chats"));
}

#[tokio::test]
async fn cli_backfill_tracks_first_then_explains_missing_credentials_and_validates_pacing() {
    let (_directory, url, store) = store_with_chats().await;
    store.close().await;
    let id = private().get().to_string();

    // Invalid pacing config is rejected before tracking changes anything.
    let out = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("DATABASE_URL", &url)
        .env("SYNC_PAGE_DELAY_MS", "70000")
        .args(["chats", "track", &id, "--backfill"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("SYNC_PAGE_DELAY_MS"));
    let out = cli(&url, &["--output", "json", "chats", "get", &id]);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["tracked"],
        false
    );

    // No Telegram credentials: tracking is saved, the backfill fails non-zero with next steps.
    let out = cli(&url, &["chats", "track", &id, "--backfill"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Tracking chat"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("is tracked") && stderr.contains("sync chat"),
        "{stderr}"
    );
    let out = cli(&url, &["--output", "json", "chats", "get", &id]);
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["tracked"],
        true
    );

    // Without --backfill nothing changes: success and no sync attempt.
    let out = cli(&url, &["chats", "track", &id]);
    assert!(out.status.success());
}

#[tokio::test]
async fn rest_backfill_runs_a_real_history_job_on_the_sqlite_store() {
    let rt = runtime().await;
    let service = router_with_sync(
        Arc::new(application(&rt.store)),
        Some(rt.coordinator.clone()),
    );
    let (status, body) = send(
        &service,
        "PUT",
        &format!("/api/v1/chats/{}/tracking?backfill=true", private().get()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let job = rt
        .coordinator
        .wait_job(body["backfill_job_id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(job.state, SyncJobState::Succeeded);
    assert_eq!(*rt.gateway.fetched.lock().unwrap(), [private()]);
    rt.coordinator.shutdown().await.unwrap();
}

#[tokio::test]
async fn migration_0004_keeps_job_history_and_allows_the_rate_limited_state() {
    let directory = tempfile::tempdir().unwrap();
    let url = url(&directory);
    let old = directory.path().join("old-migrations");
    std::fs::create_dir(&old).unwrap();
    for name in [
        "0001_archive.sql",
        "0002_telegram_account_deletions.sql",
        "0003_chat_tracking.sql",
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
    for sql in [
        "INSERT INTO chats(id, kind, created_at, updated_at) VALUES (42, 'private', 1, 1)",
        "INSERT INTO sync_jobs(id, scope, chat_id, state, created_at) VALUES ('old', 'chat', 42, 'failed', 1)",
        "INSERT INTO sync_job_chats(job_id, chat_id, state, committed_count) VALUES ('old', 42, 'failed', 9)",
    ] {
        sqlx::query(sql).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let store = SqliteStore::connect(&url).await.unwrap();
    let chat = ChatId::from_marked(42).unwrap();
    let old = SyncRepository::get_job(&store, "old")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.state, SyncJobState::Failed);
    let progress = store.list_chat_progress("old").await.unwrap();
    assert_eq!(progress[0].committed_messages, 9);

    let mut job = SyncJob {
        id: "new".into(),
        scope: SyncScope::Chat(chat),
        state: SyncJobState::Queued,
        created_at: Utc::now(),
        started_at: None,
        completed_at: None,
        summary_error: None,
    };
    store.save_job(job.clone()).await.unwrap();
    job.state = SyncJobState::Running;
    store.save_job(job.clone()).await.unwrap();
    store
        .write_batch(IngestBatch {
            job_progress: Some(SyncChatProgress {
                job_id: "new".into(),
                chat_id: chat,
                state: SyncJobState::RateLimited,
                committed_messages: 3,
                summary_error: Some("Telegram rate limit: retry after 900 s".into()),
            }),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    job.state = SyncJobState::RateLimited;
    job.summary_error = Some("Telegram rate limit: retry after 900 s".into());
    store.save_job(job).await.unwrap();
    let saved = SyncRepository::get_job(&store, "new")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.state, SyncJobState::RateLimited);
    let service = router_with_sync(Arc::new(application(&Arc::new(store))), None);
    let (_, body) = send(&service, "GET", "/api/v1/status").await;
    let states: Vec<_> = body["sync_jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["state"].as_str().unwrap().to_owned())
        .collect();
    assert!(states.contains(&"rate_limited".to_owned()), "{states:?}");
}

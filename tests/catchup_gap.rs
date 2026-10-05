use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tgarchive::{
    application::{
        ArchiveWriter, ChatRepository, HistoryBoundary, HistoryPage, IngestBatch, IngestRecord,
        MessageRepository, MessageSource, PageSize, SyncJob, SyncJobState, SyncRepository,
        SyncScope, TelegramError, TelegramGateway, ingestion_worker,
        sync::{CancellationToken, SyncEngine},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
    infrastructure::persistence::sqlite::SqliteStore,
};
use tokio::sync::Mutex;

fn chat_id() -> ChatId {
    ChatId::from_telegram(ChatKind::Private, 7).unwrap()
}

fn chat() -> Chat {
    Chat {
        id: chat_id(),
        kind: ChatKind::Private,
        title: Some("c".into()),
        username: None,
        tracked: false,
    }
}

fn message(id: i64) -> Message {
    let timestamp = DateTime::<Utc>::from_timestamp(1_700_000_000 + id, 0).unwrap();
    Message {
        post_author: None,
        forward: None,
        id: MessageId::new(id).unwrap(),
        chat_id: chat_id(),
        sender_id: None,
        timestamp,
        edited_at: None,
        collected_at: timestamp,
        text: Some(format!("m{id}")),
        reply_to: None,
        attachments: vec![],
    }
}

fn record(id: i64, source: MessageSource) -> IngestRecord {
    IngestRecord {
        event: MessageEvent::Created(message(id)),
        source,
    }
}

/// Fake chat whose messages are the IDs in `ids`; honours before/after cursors like Telegram.
struct Gateway {
    ids: Mutex<Vec<i64>>,
    requests: Mutex<usize>,
}

impl Gateway {
    fn new(ids: impl IntoIterator<Item = i64>) -> Arc<Self> {
        Arc::new(Self {
            ids: Mutex::new(ids.into_iter().collect()),
            requests: Mutex::new(0),
        })
    }
}

#[async_trait]
impl TelegramGateway for Gateway {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        Ok(vec![chat()])
    }
    async fn fetch_history(
        &self,
        _: ChatId,
        boundary: HistoryBoundary,
        size: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        *self.requests.lock().await += 1;
        let ids = self.ids.lock().await.clone();
        let size = size.get() as usize;
        let id = |raw: i64| MessageId::new(raw).unwrap();
        let mut page = HistoryPage {
            chats: vec![chat()],
            senders: vec![],
            records: vec![],
            next_before_message_id: None,
            next_after_message_id: None,
            exhausted: false,
        };
        if let Some(after) = boundary.after_message_id {
            let mut newer: Vec<i64> = ids.into_iter().filter(|i| *i > after.get()).collect();
            newer.sort();
            page.exhausted = newer.len() <= size;
            newer.truncate(size);
            page.next_after_message_id = newer.last().copied().map(id);
            page.records = newer
                .into_iter()
                .map(|i| record(i, MessageSource::History))
                .collect();
        } else {
            let before = boundary.before_message_id.map_or(i64::MAX, MessageId::get);
            let mut older: Vec<i64> = ids.into_iter().filter(|i| *i < before).collect();
            older.sort_by(|a, b| b.cmp(a));
            page.exhausted = older.len() <= size;
            older.truncate(size);
            page.next_before_message_id = older.last().copied().map(id);
            page.records = older
                .into_iter()
                .map(|i| record(i, MessageSource::History))
                .collect();
        }
        Ok(page)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<SqliteStore>,
    gateway: Arc<Gateway>,
    engine: SyncEngine,
}

async fn fixture(ids: impl IntoIterator<Item = i64>, page: u16) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("a.db").display());
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    store
        .write_batch(IngestBatch {
            chats: vec![chat()],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    store.set_tracked(chat_id(), true).await.unwrap();
    let gateway = Gateway::new(ids);
    let (sink, worker) = ingestion_worker::spawn(store.clone(), 4);
    drop(worker);
    let engine = SyncEngine::new(
        gateway.clone(),
        store.clone(),
        store.clone(),
        sink,
        PageSize::new(page).unwrap(),
    );
    Fixture {
        _dir: dir,
        store,
        gateway,
        engine,
    }
}

impl Fixture {
    async fn sync(&self, job: &str, cancel: &CancellationToken) -> u64 {
        self.store
            .save_job(SyncJob {
                id: job.into(),
                scope: SyncScope::Chat(chat_id()),
                state: SyncJobState::Running,
                created_at: Utc::now(),
                started_at: None,
                completed_at: None,
                summary_error: None,
            })
            .await
            .unwrap();
        self.engine.sync_chat(job, chat_id(), cancel).await.unwrap()
    }
}

async fn archived(store: &SqliteStore, ids: impl IntoIterator<Item = i64>) -> Vec<i64> {
    let mut present = vec![];
    for raw in ids {
        if MessageRepository::get(store, chat_id(), MessageId::new(raw).unwrap(), false)
            .await
            .unwrap()
            .is_some()
        {
            present.push(raw);
        }
    }
    present
}

async fn catchup_after(store: &SqliteStore) -> Option<i64> {
    store
        .get_checkpoint(chat_id())
        .await
        .unwrap()
        .and_then(|c| c.catchup_after_id)
        .map(MessageId::get)
}

#[tokio::test]
async fn history_sync_then_new_messages_then_catch_up_archives_the_gap() {
    let f = fixture(1..=1000, 100).await;
    let cancel = CancellationToken::new();
    f.sync("j1", &cancel).await;
    assert_eq!(catchup_after(&f.store).await, Some(1000));

    f.gateway.ids.lock().await.extend(1001..=1050);
    // `serve` starts later: baseline pass must not jump to 1050.
    f.engine.baseline_new_tracked(&cancel).await.unwrap();
    assert_eq!(catchup_after(&f.store).await, Some(1000));
    f.engine.catch_up_all(&cancel).await.unwrap();
    assert_eq!(archived(&f.store, 1001..=1050).await.len(), 50);
    assert_eq!(catchup_after(&f.store).await, Some(1050));
}

#[tokio::test]
async fn sync_chat_on_complete_history_fetches_newer_messages() {
    let f = fixture(1..=30, 10).await;
    let cancel = CancellationToken::new();
    f.sync("j1", &cancel).await;
    f.gateway.ids.lock().await.extend(31..=45);
    let n = f.sync("j2", &cancel).await;
    assert_eq!(n, 15);
    assert_eq!(archived(&f.store, 31..=45).await.len(), 15);
    assert_eq!(f.sync("j3", &cancel).await, 0);
}

#[tokio::test]
async fn legacy_complete_checkpoint_without_baseline_uses_archived_max() {
    let f = fixture(1..=10, 5).await;
    let cancel = CancellationToken::new();
    f.sync("j1", &cancel).await;
    // Simulate a pre-fix database: history complete, no catch-up baseline.
    sqlx_clear_baseline(&f).await;
    assert_eq!(catchup_after(&f.store).await, None);
    f.gateway.ids.lock().await.extend(11..=20);
    f.engine.baseline_new_tracked(&cancel).await.unwrap();
    assert_eq!(archived(&f.store, 11..=20).await.len(), 10);
    assert_eq!(catchup_after(&f.store).await, Some(20));
}

async fn sqlx_clear_baseline(f: &Fixture) {
    // The store merges NULL/MAX monotonically, so rewrite the row through a fresh pool.
    let url = format!("sqlite://{}", f._dir.path().join("a.db").display());
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("UPDATE chat_sync_state SET catchup_after_id=NULL")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn baseline_with_existing_archive_uses_archived_max() {
    let f = fixture(1..=50, 10).await;
    f.store
        .write_batch(IngestBatch {
            records: (1..=5)
                .map(|i| record(i, MessageSource::Realtime))
                .collect(),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    f.engine.baseline_new_tracked(&cancel).await.unwrap();
    assert_eq!(archived(&f.store, 6..=50).await.len(), 45);
    assert_eq!(catchup_after(&f.store).await, Some(50));
}

#[tokio::test]
async fn fresh_chat_without_archive_baselines_at_telegram_newest() {
    let f = fixture(1..=50, 10).await;
    let cancel = CancellationToken::new();
    f.engine.baseline_new_tracked(&cancel).await.unwrap();
    assert_eq!(catchup_after(&f.store).await, Some(50));
    assert!(archived(&f.store, 1..=50).await.is_empty());
}

#[tokio::test]
async fn empty_chat_baselines_at_zero_and_later_messages_are_caught_up() {
    let f = fixture([], 10).await;
    let cancel = CancellationToken::new();
    f.engine.baseline_new_tracked(&cancel).await.unwrap();
    assert_eq!(catchup_after(&f.store).await, Some(0));
    f.gateway.ids.lock().await.extend(1..=3);
    f.engine.catch_up_all(&cancel).await.unwrap();
    assert_eq!(archived(&f.store, 1..=3).await.len(), 3);
    assert!(*f.gateway.requests.lock().await > 0);
}

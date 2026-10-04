use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use tgarchive::{
    application::{
        ArchiveWriter, ChatCheckpoint, ChatRepository, HistoryBoundary, HistoryPage, IngestBatch,
        IngestRecord, MessageSource, PageSize, RepositoryError, SyncChatProgress, SyncJob,
        SyncJobState, SyncRepository, SyncScope, TelegramError, TelegramGateway, ingestion_worker,
        pacer::RatePacer,
        sync::{CancellationToken, SyncCoordinator, SyncEngine},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
};
use tokio::{sync::Mutex, time::Instant};

#[derive(Default)]
struct Store {
    checkpoints: Mutex<HashMap<ChatId, ChatCheckpoint>>,
    progress: Mutex<Vec<SyncChatProgress>>,
    jobs: Mutex<HashMap<String, SyncJob>>,
    chats: Mutex<HashMap<ChatId, Chat>>,
    chat_errors: Mutex<Vec<(ChatId, String)>>,
}

#[async_trait]
impl ArchiveWriter for Store {
    async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
        if let Some((chat, checkpoint)) = batch.checkpoint {
            self.checkpoints.lock().await.insert(chat, checkpoint);
        }
        if let Some(progress) = batch.job_progress {
            self.progress.lock().await.push(progress);
        }
        if let Some(error) = batch.chat_error {
            self.chat_errors.lock().await.push(error);
        }
        Ok(())
    }
}
#[async_trait]
impl ChatRepository for Store {
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError> {
        Ok(self.chats.lock().await.get(&id).cloned())
    }
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
        Ok(self.chats.lock().await.values().cloned().collect())
    }
    async fn save_refresh(&self, _: Vec<Chat>) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn set_tracked(&self, _: ChatId, _: bool) -> Result<Option<Chat>, RepositoryError> {
        Ok(None)
    }
}
#[async_trait]
impl SyncRepository for Store {
    async fn get_checkpoint(&self, id: ChatId) -> Result<Option<ChatCheckpoint>, RepositoryError> {
        Ok(self.checkpoints.lock().await.get(&id).cloned())
    }
    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError> {
        self.jobs.lock().await.insert(job.id.clone(), job);
        Ok(())
    }
    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError> {
        Ok(self.jobs.lock().await.get(id).cloned())
    }
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        Ok(self.jobs.lock().await.values().cloned().collect())
    }
    async fn list_chat_progress(
        &self,
        job: &str,
    ) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        Ok(self
            .progress
            .lock()
            .await
            .iter()
            .filter(|p| p.job_id == job)
            .cloned()
            .collect())
    }
    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

/// Per-chat scripted responses; records when each fetch started (virtual time) and how many
/// fetches were in flight at once.
struct Gateway {
    start: Instant,
    scripts: StdMutex<HashMap<ChatId, VecDeque<Result<HistoryPage, TelegramError>>>>,
    log: StdMutex<Vec<(Duration, ChatId, HistoryBoundary)>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    latency: Duration,
}
#[async_trait]
impl TelegramGateway for Gateway {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        Ok(vec![])
    }
    async fn fetch_history(
        &self,
        chat: ChatId,
        boundary: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        self.log
            .lock()
            .unwrap()
            .push((self.start.elapsed(), chat, boundary));
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.latency).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.scripts
            .lock()
            .unwrap()
            .get_mut(&chat)
            .and_then(VecDeque::pop_front)
            .expect("scripted response")
    }
}
impl Gateway {
    fn new(latency: Duration) -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            scripts: Default::default(),
            log: Default::default(),
            in_flight: Default::default(),
            max_in_flight: Default::default(),
            latency,
        })
    }
    fn script(&self, chat: ChatId, items: Vec<Result<HistoryPage, TelegramError>>) {
        self.scripts.lock().unwrap().insert(chat, items.into());
    }
    fn times_ms(&self) -> Vec<u128> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .map(|(at, ..)| at.as_millis())
            .collect()
    }
}

fn chat_id(raw: i64) -> ChatId {
    ChatId::from_telegram(ChatKind::Private, raw).unwrap()
}
async fn tracked(store: &Store, ids: &[ChatId]) {
    for id in ids {
        let id = *id;
        store.chats.lock().await.insert(
            id,
            Chat {
                id,
                kind: ChatKind::Private,
                title: None,
                username: None,
                tracked: true,
            },
        );
    }
}
fn page(next: Option<i64>, exhausted: bool) -> HistoryPage {
    HistoryPage {
        chats: vec![],
        senders: vec![],
        records: vec![],
        next_before_message_id: next.map(|id| MessageId::new(id).unwrap()),
        next_after_message_id: None,
        exhausted,
    }
}
fn page_with_records(chat: ChatId, ids: &[i64], next: i64) -> HistoryPage {
    let mut page = page(Some(next), false);
    page.records = ids
        .iter()
        .map(|id| IngestRecord {
            event: MessageEvent::Created(Message {
                id: MessageId::new(*id).unwrap(),
                chat_id: chat,
                sender_id: None,
                timestamp: Utc::now(),
                edited_at: None,
                collected_at: Utc::now(),
                text: Some("x".into()),
                reply_to: None,
                attachments: vec![],
            }),
            source: MessageSource::History,
        })
        .collect();
    page
}
fn flood(secs: u64) -> Result<HistoryPage, TelegramError> {
    Err(TelegramError::FloodWait {
        retry_after_seconds: secs,
    })
}

fn engine(
    store: &Arc<Store>,
    gateway: &Arc<Gateway>,
    delay_ms: u64,
    ceiling_secs: u64,
) -> Arc<SyncEngine> {
    let (sink, writer) = ingestion_worker::spawn(store.clone(), 8);
    drop(writer);
    Arc::new(
        SyncEngine::new(
            gateway.clone(),
            store.clone(),
            store.clone(),
            sink,
            PageSize::DEFAULT,
        )
        .with_rate_control(
            Arc::new(RatePacer::new(Duration::from_millis(delay_ms))),
            Duration::from_secs(ceiling_secs),
        ),
    )
}

#[tokio::test(start_paused = true)]
async fn history_pages_are_spaced_by_the_pacer() {
    let (store, gateway) = (Arc::new(Store::default()), Gateway::new(Duration::ZERO));
    let a = chat_id(1);
    gateway.script(
        a,
        vec![
            Ok(page(Some(90), false)),
            Ok(page(Some(80), false)),
            Ok(page(Some(70), false)),
            Ok(page(None, true)),
        ],
    );
    let engine = engine(&store, &gateway, 1000, 300);
    engine
        .sync_chat("j", a, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(gateway.times_ms(), [0, 1000, 2000, 3000]);
}

#[tokio::test(start_paused = true)]
async fn zero_delay_disables_pacing() {
    let (store, gateway) = (Arc::new(Store::default()), Gateway::new(Duration::ZERO));
    let a = chat_id(1);
    gateway.script(
        a,
        vec![
            Ok(page(Some(90), false)),
            Ok(page(Some(80), false)),
            Ok(page(None, true)),
        ],
    );
    let engine = engine(&store, &gateway, 0, 300);
    engine
        .sync_chat("j", a, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(gateway.times_ms(), [0, 0, 0]);
}

#[tokio::test(start_paused = true)]
async fn catch_up_and_history_share_one_pacer() {
    let (store, gateway) = (Arc::new(Store::default()), Gateway::new(Duration::ZERO));
    let (a, b) = (chat_id(1), chat_id(2));
    tracked(&store, &[b]).await;
    gateway.script(a, vec![Ok(page(Some(90), false)), Ok(page(None, true))]);
    // Catch-up on an unbaselined chat: one probe for the newest message.
    gateway.script(b, vec![Ok(page(Some(10), true))]);
    let engine = engine(&store, &gateway, 1000, 300);
    let cancel = CancellationToken::new();
    let (history, catch_up) = tokio::join!(
        engine.sync_chat("j", a, &cancel),
        engine.catch_up_all(&cancel)
    );
    history.unwrap();
    catch_up.unwrap();
    let times = gateway.times_ms();
    assert_eq!(times, [0, 1000, 2000]);
    assert!(
        gateway
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|(_, chat, _)| *chat == b),
        "catch-up probe went through the same pacer"
    );
}

#[tokio::test(start_paused = true)]
async fn flood_wait_slows_the_pacer_and_is_reported() {
    let (store, gateway) = (Arc::new(Store::default()), Gateway::new(Duration::ZERO));
    let a = chat_id(1);
    gateway.script(
        a,
        vec![flood(1), Ok(page(Some(90), false)), Ok(page(None, true))],
    );
    let engine = engine(&store, &gateway, 1000, 300);
    engine
        .sync_chat("j", a, &CancellationToken::new())
        .await
        .unwrap();
    // 0: FLOOD_WAIT 1s -> retry at 1s; interval doubled to 2s -> next page at 3s.
    assert_eq!(gateway.times_ms(), [0, 1000, 3000]);
    let status = engine.pacer().status();
    assert_eq!(status.interval_ms, 2000);
    assert_eq!(status.base_interval_ms, 1000);
    assert_eq!(status.last_flood_wait_secs, Some(1));
    assert!(status.last_flood_at.is_some());
}

#[tokio::test(start_paused = true)]
async fn two_history_jobs_never_fetch_concurrently() {
    let store = Arc::new(Store::default());
    let gateway = Gateway::new(Duration::from_millis(10));
    let (a, b) = (chat_id(1), chat_id(2));
    tracked(&store, &[a, b]).await;
    for chat in [a, b] {
        gateway.script(chat, vec![Ok(page(Some(90), false)), Ok(page(None, true))]);
    }
    let coordinator = SyncCoordinator::spawn(engine(&store, &gateway, 0, 300), store.clone(), 4);
    let first = coordinator.submit(SyncScope::Chat(a)).await.unwrap();
    let second = coordinator.submit(SyncScope::Chat(b)).await.unwrap();
    let first = coordinator.wait_job(&first.id).await.unwrap();
    let second = coordinator.wait_job(&second.id).await.unwrap();
    assert_eq!(first.state, SyncJobState::Succeeded);
    assert_eq!(second.state, SyncJobState::Succeeded);
    assert_eq!(gateway.max_in_flight.load(Ordering::SeqCst), 1);
    assert!(second.started_at >= first.completed_at);
    coordinator.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn flood_wait_above_ceiling_pauses_the_job_and_a_later_job_resumes() {
    let store = Arc::new(Store::default());
    let gateway = Gateway::new(Duration::ZERO);
    let a = chat_id(1);
    tracked(&store, &[a]).await;
    gateway.script(
        a,
        vec![Ok(page_with_records(a, &[100, 99], 90)), flood(4000)],
    );
    let coordinator = SyncCoordinator::spawn(engine(&store, &gateway, 0, 300), store.clone(), 4);
    let started = Instant::now();
    let job = coordinator.submit(SyncScope::Chat(a)).await.unwrap();
    let job = coordinator.wait_job(&job.id).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "must not sleep 4000 s"
    );
    assert_eq!(job.state, SyncJobState::RateLimited);
    assert_eq!(
        job.summary_error.as_deref(),
        Some("Telegram rate limit: retry after 4000 s")
    );
    let progress = store.list_chat_progress(&job.id).await.unwrap();
    let last = progress.last().unwrap();
    assert_eq!(last.state, SyncJobState::RateLimited);
    assert_eq!(last.committed_messages, 2);
    let checkpoint = store.get_checkpoint(a).await.unwrap().unwrap();
    assert_eq!(
        checkpoint.history_before_id,
        Some(MessageId::new(90).unwrap())
    );
    assert!(!checkpoint.history_complete);

    gateway.script(a, vec![Ok(page(None, true))]);
    let resumed = coordinator.submit(SyncScope::Chat(a)).await.unwrap();
    let resumed = coordinator.wait_job(&resumed.id).await.unwrap();
    assert_eq!(resumed.state, SyncJobState::Succeeded);
    let resumed_from = gateway
        .log
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .2
        .before_message_id;
    assert_eq!(
        resumed_from,
        Some(MessageId::new(90).unwrap()),
        "resume continues from the committed checkpoint"
    );
    coordinator.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn exhausted_flood_retries_also_pause_the_job() {
    let store = Arc::new(Store::default());
    let gateway = Gateway::new(Duration::ZERO);
    let a = chat_id(1);
    tracked(&store, &[a]).await;
    gateway.script(a, (0..6).map(|_| flood(1)).collect());
    let coordinator = SyncCoordinator::spawn(engine(&store, &gateway, 0, 300), store.clone(), 4);
    let job = coordinator.submit(SyncScope::Chat(a)).await.unwrap();
    let job = coordinator.wait_job(&job.id).await.unwrap();
    assert_eq!(job.state, SyncJobState::RateLimited);
    assert_eq!(gateway.log.lock().unwrap().len(), 6);
    coordinator.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn catch_up_skips_a_chat_on_a_flood_wait_above_the_ceiling() {
    let store = Arc::new(Store::default());
    let gateway = Gateway::new(Duration::ZERO);
    let (a, b) = (chat_id(1), chat_id(2));
    tracked(&store, &[a, b]).await;
    gateway.script(a, vec![flood(4000)]);
    gateway.script(b, vec![Ok(page(Some(10), true))]);
    let engine = engine(&store, &gateway, 0, 300);
    let started = Instant::now();
    let summary = engine
        .catch_up_all(&CancellationToken::new())
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(summary.failures.len(), 1);
    assert_eq!(summary.caught_up_chats, 1);
    assert_eq!(store.chat_errors.lock().await.len(), 1);
    assert_eq!(engine.pacer().status().last_flood_wait_secs, Some(4000));
}

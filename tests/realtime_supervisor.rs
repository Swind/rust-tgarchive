use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use chrono::DateTime;
use telegram_message_archive::{
    application::{
        ApplicationError, ArchiveWriter, ChatCheckpoint, ChatRepository, HistoryBoundary,
        HistoryPage, IngestBatch, IngestRecord, MessageSource, PageSize, RepositoryError,
        SyncChatProgress, SyncJob, SyncRepository, TelegramError, TelegramGateway,
        ingestion_worker,
        realtime::{
            RealtimeSource, RealtimeSourceError, ReconnectBackoff, supervise_realtime,
            supervise_realtime_with_status,
        },
        services::{CollectorStatusHandle, ComponentState, ComponentStatus},
        sync::{CancellationToken, SyncEngine},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
};
use tokio::sync::Mutex;

fn chat_id() -> ChatId {
    ChatId::from_telegram(ChatKind::Private, 7).unwrap()
}

fn message(id: i64) -> Message {
    let timestamp = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    Message {
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

#[derive(Default)]
struct Store {
    checkpoints: Mutex<HashMap<ChatId, ChatCheckpoint>>,
    batches: Mutex<Vec<IngestBatch>>,
    fail_write_number: Mutex<Option<usize>>,
}

#[async_trait]
impl ArchiveWriter for Store {
    async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
        let number = self.batches.lock().await.len() + 1;
        if *self.fail_write_number.lock().await == Some(number) {
            return Err(RepositoryError::Unavailable(
                "injected write failure".into(),
            ));
        }
        if let Some((chat, checkpoint)) = batch.checkpoint.clone() {
            self.checkpoints.lock().await.insert(chat, checkpoint);
        }
        self.batches.lock().await.push(batch);
        Ok(())
    }
}

#[async_trait]
impl ChatRepository for Store {
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError> {
        Ok((id == chat_id()).then(chat))
    }
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
        Ok(vec![chat()])
    }
    async fn save_refresh(&self, _: Vec<Chat>) -> Result<(), RepositoryError> {
        Ok(())
    }
}

fn chat() -> Chat {
    Chat {
        id: chat_id(),
        kind: ChatKind::Private,
        title: None,
        username: None,
    }
}

#[async_trait]
impl SyncRepository for Store {
    async fn get_checkpoint(&self, id: ChatId) -> Result<Option<ChatCheckpoint>, RepositoryError> {
        Ok(self.checkpoints.lock().await.get(&id).cloned())
    }
    async fn save_job(&self, _: SyncJob) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn get_job(&self, _: &str) -> Result<Option<SyncJob>, RepositoryError> {
        Ok(None)
    }
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        Ok(vec![])
    }
    async fn list_chat_progress(&self, _: &str) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        Ok(vec![])
    }
    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

/// Telegram with an ordered list of message IDs; `arrive_after_fetch` adds new messages
/// once that many fetches have happened, simulating live traffic during catch-up.
struct Gateway {
    ids: Mutex<Vec<i64>>,
    boundaries: Mutex<Vec<HistoryBoundary>>,
    arrive_after_fetch: Mutex<Option<(usize, Vec<i64>)>>,
    fail_with: Mutex<VecDeque<TelegramError>>,
}

impl Gateway {
    fn new(ids: impl IntoIterator<Item = i64>) -> Arc<Self> {
        Arc::new(Self {
            ids: Mutex::new(ids.into_iter().collect()),
            boundaries: Mutex::default(),
            arrive_after_fetch: Mutex::default(),
            fail_with: Mutex::default(),
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
        page_size: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        if let Some(error) = self.fail_with.lock().await.pop_front() {
            return Err(error);
        }
        let fetches = {
            let mut boundaries = self.boundaries.lock().await;
            boundaries.push(boundary.clone());
            boundaries.len()
        };
        let mut ids = self.ids.lock().await;
        let size = usize::from(page_size.get());
        let page = if let Some(after) = boundary.after_message_id {
            let rest = ids
                .iter()
                .copied()
                .filter(|id| *id > after.get())
                .collect::<Vec<_>>();
            let page = rest.iter().copied().take(size).collect::<Vec<_>>();
            (page, rest.len() <= size, true)
        } else {
            let mut rest = ids.clone();
            rest.reverse();
            let page = rest.iter().copied().take(size).collect::<Vec<_>>();
            (page, rest.len() <= size, false)
        };
        let (page, exhausted, ascending) = page;
        let last = page.last().map(|id| MessageId::new(*id).unwrap());
        let result = HistoryPage {
            chats: vec![chat()],
            senders: vec![],
            records: page
                .iter()
                .map(|id| IngestRecord {
                    event: MessageEvent::Created(message(*id)),
                    source: MessageSource::History,
                })
                .collect(),
            next_before_message_id: if ascending { None } else { last },
            next_after_message_id: if ascending { last } else { None },
            exhausted,
        };
        let mut arrival = self.arrive_after_fetch.lock().await;
        if arrival.as_ref().is_some_and(|(after, _)| fetches >= *after) {
            ids.extend(arrival.take().unwrap().1);
        }
        Ok(result)
    }
}

fn engine(store: &Arc<Store>, gateway: Arc<Gateway>, page_size: u16) -> SyncEngine {
    let (sink, worker) = ingestion_worker::spawn(store.clone(), 2);
    drop(worker);
    SyncEngine::new(
        gateway,
        store.clone(),
        store.clone(),
        sink,
        PageSize::new(page_size).unwrap(),
    )
}

async fn seed_boundary(store: &Store, after: i64) {
    store.checkpoints.lock().await.insert(
        chat_id(),
        ChatCheckpoint {
            history_before_id: Some(MessageId::new(5).unwrap()),
            history_complete: true,
            catchup_after_id: Some(MessageId::new(after).unwrap()),
        },
    );
}

fn committed_ids(batches: &[IngestBatch]) -> Vec<Vec<i64>> {
    batches
        .iter()
        .map(|batch| {
            batch
                .records
                .iter()
                .map(|record| match &record.event {
                    MessageEvent::Created(message) => message.id.get(),
                    _ => unreachable!(),
                })
                .collect()
        })
        .collect()
}

#[tokio::test]
async fn catch_up_commits_ascending_pages_and_advances_boundary_per_page() {
    let store = Arc::new(Store::default());
    seed_boundary(&store, 10).await;
    let gateway = Gateway::new(1..=25);
    let engine = engine(&store, gateway.clone(), 5);

    let count = engine
        .catch_up_chat(chat_id(), &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(count, 15);
    let batches = store.batches.lock().await;
    assert_eq!(
        committed_ids(&batches),
        vec![
            vec![11, 12, 13, 14, 15],
            vec![16, 17, 18, 19, 20],
            vec![21, 22, 23, 24, 25]
        ]
    );
    let boundaries = batches
        .iter()
        .map(|batch| {
            batch
                .checkpoint
                .as_ref()
                .unwrap()
                .1
                .catchup_after_id
                .unwrap()
                .get()
        })
        .collect::<Vec<_>>();
    assert_eq!(boundaries, vec![15, 20, 25]);
    let requested = gateway
        .boundaries
        .lock()
        .await
        .iter()
        .filter_map(|boundary| boundary.after_message_id.map(MessageId::get))
        .collect::<Vec<_>>();
    assert_eq!(requested, vec![10, 15, 20]);
    let checkpoint = store.checkpoints.lock().await[&chat_id()].clone();
    assert!(checkpoint.history_complete, "history fields are preserved");
}

#[tokio::test]
async fn catch_up_upper_bound_is_fixed_for_the_round_and_next_round_continues() {
    let store = Arc::new(Store::default());
    seed_boundary(&store, 10).await;
    let gateway = Gateway::new(11..=20);
    *gateway.arrive_after_fetch.lock().await = Some((2, (21..=30).collect()));
    let engine = engine(&store, gateway.clone(), 8);

    engine
        .catch_up_chat(chat_id(), &CancellationToken::new())
        .await
        .unwrap();

    {
        let batches = store.batches.lock().await;
        assert_eq!(
            committed_ids(&batches),
            vec![(11..=18).collect::<Vec<_>>(), vec![19, 20]],
            "live arrivals after the bound are not part of this round"
        );
    }
    assert_eq!(
        store.checkpoints.lock().await[&chat_id()].catchup_after_id,
        Some(MessageId::new(20).unwrap())
    );

    engine
        .catch_up_chat(chat_id(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        store.checkpoints.lock().await[&chat_id()].catchup_after_id,
        Some(MessageId::new(30).unwrap())
    );
}

#[tokio::test]
async fn catch_up_does_not_advance_boundary_when_a_page_commit_fails() {
    let store = Arc::new(Store::default());
    seed_boundary(&store, 10).await;
    *store.fail_write_number.lock().await = Some(2);
    let engine = engine(&store, Gateway::new(1..=25), 5);

    let error = engine
        .catch_up_chat(chat_id(), &CancellationToken::new())
        .await
        .unwrap_err();

    assert!(matches!(error, ApplicationError::RepositoryUnavailable(_)));
    assert_eq!(
        store.checkpoints.lock().await[&chat_id()].catchup_after_id,
        Some(MessageId::new(15).unwrap()),
        "only the committed first page moved the boundary"
    );
}

#[tokio::test]
async fn catch_up_without_baseline_starts_at_the_round_bound_and_writes_no_messages() {
    let store = Arc::new(Store::default());
    let engine = engine(&store, Gateway::new(1..=9), 5);

    let count = engine
        .catch_up_chat(chat_id(), &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(count, 0);
    let checkpoint = store.checkpoints.lock().await[&chat_id()].clone();
    assert_eq!(
        checkpoint.catchup_after_id,
        Some(MessageId::new(9).unwrap())
    );
    assert!(store.batches.lock().await[0].records.is_empty());
}

#[tokio::test]
async fn catch_up_all_reports_telegram_failures_per_chat_without_aborting() {
    let store = Arc::new(Store::default());
    seed_boundary(&store, 10).await;
    let gateway = Gateway::new(1..=12);
    gateway
        .fail_with
        .lock()
        .await
        .push_back(TelegramError::Unauthorized);
    let engine = engine(&store, gateway, 5);

    let summary = engine
        .catch_up_all(&CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(summary.failures.len(), 1);
    assert_eq!(summary.caught_up_chats, 0);
}

struct Source {
    results: Mutex<VecDeque<Result<(), RealtimeSourceError>>>,
    runs: AtomicUsize,
    entered: tokio::sync::Notify,
}

impl Source {
    fn new(results: Vec<Result<(), RealtimeSourceError>>) -> Arc<Self> {
        Arc::new(Self {
            results: Mutex::new(results.into()),
            runs: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
        })
    }
}

#[async_trait]
impl RealtimeSource for Source {
    async fn run_once(&self, cancel: CancellationToken) -> Result<(), RealtimeSourceError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        match self.results.lock().await.pop_front() {
            Some(result) => result,
            None => {
                cancel.cancelled().await;
                Ok(())
            }
        }
    }
}

fn dropped() -> RealtimeSourceError {
    RealtimeSourceError::Transient("dropped".into())
}

fn backoff(initial: Duration, max: Duration) -> ReconnectBackoff {
    ReconnectBackoff {
        initial,
        max,
        healthy_after: Duration::from_secs(3600),
    }
}

#[tokio::test(start_paused = true)]
async fn dropped_stream_reconnects_with_capped_backoff_and_catches_up_each_time() {
    let store = Arc::new(Store::default());
    seed_boundary(&store, 10).await;
    let gateway = Gateway::new(1..=12);
    let engine = Arc::new(engine(&store, gateway.clone(), 5));
    let source = Source::new(vec![Err(dropped()), Err(dropped()), Err(dropped())]);
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let (source, engine, cancel) = (source.clone(), engine.clone(), cancel.clone());
        async move {
            supervise_realtime(
                source.as_ref(),
                &engine,
                backoff(Duration::from_secs(1), Duration::from_secs(2)),
                cancel,
            )
            .await
        }
    });

    let started = tokio::time::Instant::now();
    while source.runs.load(Ordering::SeqCst) < 4 {
        source.entered.notified().await;
    }
    // Delays between the four sessions: 1s, 2s (capped), 2s.
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    cancel.cancel();
    task.await.unwrap().unwrap();

    let catch_up_rounds = gateway
        .boundaries
        .lock()
        .await
        .iter()
        .filter(|boundary| boundary.after_message_id.is_none())
        .count();
    assert_eq!(catch_up_rounds, 4);
}

#[tokio::test(start_paused = true)]
async fn cancellation_during_backoff_returns_promptly_without_another_session() {
    let store = Arc::new(Store::default());
    let engine = Arc::new(engine(&store, Gateway::new(1..=1), 5));
    let source = Source::new(vec![Err(dropped())]);
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let (source, engine, cancel) = (source.clone(), engine.clone(), cancel.clone());
        async move {
            supervise_realtime(
                source.as_ref(),
                &engine,
                backoff(Duration::from_secs(3600), Duration::from_secs(3600)),
                cancel,
            )
            .await
        }
    });
    let started = tokio::time::Instant::now();
    source.entered.notified().await;
    cancel.cancel();

    task.await.unwrap().unwrap();
    assert_eq!(source.runs.load(Ordering::SeqCst), 1);
    assert!(started.elapsed() < Duration::from_secs(3600));
}

#[tokio::test(start_paused = true)]
async fn fatal_source_error_propagates_without_retry() {
    let store = Arc::new(Store::default());
    let engine = engine(&store, Gateway::new(1..=1), 5);
    let source = Source::new(vec![Err(RealtimeSourceError::Fatal(
        "AUTH_KEY_UNREGISTERED".into(),
    ))]);

    let error = supervise_realtime(
        source.as_ref(),
        &engine,
        ReconnectBackoff::default(),
        CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("AUTH_KEY_UNREGISTERED"));
    assert_eq!(source.runs.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn storage_failure_during_catch_up_is_fatal_and_no_session_starts() {
    let store = Arc::new(Store::default());
    seed_boundary(&store, 10).await;
    *store.fail_write_number.lock().await = Some(1);
    let engine = engine(&store, Gateway::new(1..=25), 5);
    let source = Source::new(vec![]);

    let result = supervise_realtime(
        source.as_ref(),
        &engine,
        ReconnectBackoff::default(),
        CancellationToken::new(),
    )
    .await;

    assert!(matches!(
        result,
        Err(ApplicationError::RepositoryUnavailable(_))
    ));
    assert_eq!(source.runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn empty_chat_gets_a_baseline_and_later_messages_are_caught_up() {
    let store = Arc::new(Store::default());
    let gateway = Gateway::new([]);
    let engine = engine(&store, gateway.clone(), 5);

    let count = engine
        .catch_up_chat(chat_id(), &CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(count, 0);
    assert_eq!(
        store.checkpoints.lock().await[&chat_id()].catchup_after_id,
        Some(MessageId::BEFORE_FIRST)
    );

    gateway.ids.lock().await.extend([1, 2]);
    let count = engine
        .catch_up_chat(chat_id(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        store.checkpoints.lock().await[&chat_id()].catchup_after_id,
        Some(MessageId::new(2).unwrap())
    );
}

#[tokio::test(start_paused = true)]
async fn catch_up_failure_is_recorded_with_a_sanitized_reason() {
    let store = Arc::new(Store::default());
    seed_boundary(&store, 10).await;
    let gateway = Gateway::new(1..=12);
    for _ in 0..4 {
        gateway
            .fail_with
            .lock()
            .await
            .push_back(TelegramError::Unavailable(format!(
                "peer\nnot\tfound\x07 {}",
                "x".repeat(500)
            )));
    }
    let engine = engine(&store, gateway, 5);

    let summary = engine
        .catch_up_all(&CancellationToken::new())
        .await
        .unwrap();

    let reason = &summary.failures[0].1;
    assert!(reason.chars().count() <= 200);
    assert!(!reason.contains(['\n', '\t', '\x07']));
    for _ in 0..100 {
        if !store.batches.lock().await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let batches = store.batches.lock().await;
    let (chat, recorded) = batches
        .iter()
        .find_map(|batch| batch.chat_error.clone())
        .expect("failure persisted as last_error");
    assert_eq!(chat, chat_id());
    assert_eq!(&recorded, reason);
}

#[tokio::test(start_paused = true)]
async fn collector_status_follows_supervisor_transitions() {
    let store = Arc::new(Store::default());
    let engine = Arc::new(engine(&store, Gateway::new(1..=1), 5));
    let source = Source::new(vec![Err(dropped())]);
    let status = CollectorStatusHandle::new(ComponentStatus::new(ComponentState::Starting, None));
    assert_eq!(status.get().state, ComponentState::Starting);
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let (source, engine, cancel, status) = (
            source.clone(),
            engine.clone(),
            cancel.clone(),
            status.clone(),
        );
        async move {
            supervise_realtime_with_status(
                source.as_ref(),
                &engine,
                backoff(Duration::from_secs(3600), Duration::from_secs(3600)),
                cancel,
                &status,
            )
            .await
        }
    });

    source.entered.notified().await;
    tokio::time::sleep(Duration::from_millis(1)).await;
    let reconnecting = status.get();
    assert_eq!(reconnecting.state, ComponentState::Reconnecting);
    let detail = reconnecting.detail.unwrap();
    assert!(detail.contains("attempt 1") && detail.contains("3600000 ms"));

    cancel.cancel();
    task.await.unwrap().unwrap();
    assert_eq!(status.get().state, ComponentState::Stopped);
}

#[tokio::test(start_paused = true)]
async fn collector_status_is_running_during_a_live_session_and_failed_on_fatal_error() {
    let store = Arc::new(Store::default());
    let engine = Arc::new(engine(&store, Gateway::new(1..=1), 5));
    let source = Source::new(vec![]);
    let status = CollectorStatusHandle::new(ComponentStatus::disabled());
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let (source, engine, cancel, status) = (
            source.clone(),
            engine.clone(),
            cancel.clone(),
            status.clone(),
        );
        async move {
            supervise_realtime_with_status(
                source.as_ref(),
                &engine,
                ReconnectBackoff::default(),
                cancel,
                &status,
            )
            .await
        }
    });
    source.entered.notified().await;
    assert_eq!(status.get().state, ComponentState::Running);
    cancel.cancel();
    task.await.unwrap().unwrap();

    let fatal = Source::new(vec![Err(RealtimeSourceError::Fatal(
        "AUTH_KEY\nUNREGISTERED".into(),
    ))]);
    supervise_realtime_with_status(
        fatal.as_ref(),
        &engine,
        ReconnectBackoff::default(),
        CancellationToken::new(),
        &status,
    )
    .await
    .unwrap_err();
    let failed = status.get();
    assert_eq!(failed.state, ComponentState::Failed);
    assert!(failed.detail.unwrap().contains("AUTH_KEY UNREGISTERED"));
}

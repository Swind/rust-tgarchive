use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use telegram_message_archive::{
    application::{
        ArchiveWriter, ChatCheckpoint, ChatRepository, HistoryBoundary, HistoryPage, IngestBatch,
        ListMessagesQuery, MessagePage, MessageRepository, PageSize, RepositoryError,
        SearchMessagesQuery, SyncChatProgress, SyncJob, SyncJobState, SyncRepository, SyncScope,
        TelegramError, TelegramGateway, ingestion_worker,
        sync::{CancellationToken, SyncCoordinator, SyncEngine},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageId},
};
use tokio::sync::Mutex;

#[derive(Default)]
struct Store {
    checkpoints: Mutex<HashMap<ChatId, ChatCheckpoint>>,
    progress: Mutex<Vec<SyncChatProgress>>,
    jobs: Mutex<HashMap<String, SyncJob>>,
    chats: Mutex<HashMap<ChatId, Chat>>,
    fail_writer: AtomicBool,
    fail_terminal_job: AtomicBool,
}

#[async_trait]
impl ArchiveWriter for Store {
    async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
        if self.fail_writer.load(Ordering::SeqCst) {
            return Err(RepositoryError::Unavailable(
                "injected writer failure".into(),
            ));
        }
        if let Some((chat, checkpoint)) = batch.checkpoint {
            self.checkpoints.lock().await.insert(chat, checkpoint);
        }
        if let Some(progress) = batch.job_progress {
            self.progress.lock().await.push(progress);
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
    async fn save_refresh(&self, chats: Vec<Chat>) -> Result<(), RepositoryError> {
        self.chats
            .lock()
            .await
            .extend(chats.into_iter().map(|chat| (chat.id, chat)));
        Ok(())
    }
}
#[async_trait]
impl SyncRepository for Store {
    async fn get_checkpoint(&self, id: ChatId) -> Result<Option<ChatCheckpoint>, RepositoryError> {
        Ok(self.checkpoints.lock().await.get(&id).cloned())
    }
    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError> {
        if self.fail_terminal_job.load(Ordering::SeqCst) && job.state == SyncJobState::Succeeded {
            return Err(RepositoryError::Unavailable(
                "injected terminal job write failure".into(),
            ));
        }
        self.jobs.lock().await.insert(job.id.clone(), job);
        Ok(())
    }
    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError> {
        Ok(self.jobs.lock().await.get(id).cloned())
    }
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        Ok(self.jobs.lock().await.values().cloned().collect())
    }
    async fn list_chat_progress(&self, _: &str) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        Ok(self.progress.lock().await.clone())
    }
    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}
#[async_trait]
impl MessageRepository for Store {
    async fn get(&self, _: ChatId, _: MessageId) -> Result<Option<Message>, RepositoryError> {
        Ok(None)
    }
    async fn list(&self, _: ListMessagesQuery) -> Result<MessagePage, RepositoryError> {
        unreachable!()
    }
    async fn search(&self, _: SearchMessagesQuery) -> Result<MessagePage, RepositoryError> {
        unreachable!()
    }
}

struct Gateway {
    pages: Mutex<VecDeque<Result<HistoryPage, TelegramError>>>,
    boundaries: Mutex<Vec<HistoryBoundary>>,
    chats: Mutex<Vec<Chat>>,
    fetch_started: tokio::sync::Notify,
}
#[async_trait]
impl TelegramGateway for Gateway {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        Ok(self.chats.lock().await.clone())
    }
    async fn fetch_history(
        &self,
        _: ChatId,
        boundary: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        self.boundaries.lock().await.push(boundary);
        self.fetch_started.notify_one();
        self.pages.lock().await.pop_front().expect("scripted page")
    }
}

fn chat_id() -> ChatId {
    ChatId::from_telegram(ChatKind::Private, 7).unwrap()
}
fn chat(raw: i64) -> Chat {
    let id = ChatId::from_telegram(ChatKind::Private, raw).unwrap();
    Chat {
        id,
        kind: ChatKind::Private,
        title: Some(format!("chat {raw}")),
        username: None,
    }
}

fn make_coordinator(
    store: &Arc<Store>,
    chats: Vec<Chat>,
    pages: Vec<Result<HistoryPage, TelegramError>>,
    capacity: usize,
) -> (
    Arc<SyncCoordinator>,
    tokio::task::JoinHandle<Result<(), RepositoryError>>,
    Arc<Gateway>,
) {
    let gateway = Arc::new(Gateway {
        pages: Mutex::new(pages.into()),
        boundaries: Mutex::new(vec![]),
        chats: Mutex::new(chats),
        fetch_started: tokio::sync::Notify::new(),
    });
    let (sink, writer) = ingestion_worker::spawn(store.clone(), 4);
    let engine = Arc::new(SyncEngine::new(
        gateway.clone(),
        store.clone(),
        store.clone(),
        sink,
        PageSize::DEFAULT,
    ));
    (
        SyncCoordinator::spawn(engine, store.clone(), capacity),
        writer,
        gateway,
    )
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
fn engine(store: &Arc<Store>, gateway: Arc<Gateway>) -> SyncEngine {
    let (sink, worker) = ingestion_worker::spawn(store.clone(), 2);
    drop(worker); // detached writer stops when the test runtime shuts down
    SyncEngine::new(
        gateway,
        store.clone(),
        store.clone(),
        sink,
        PageSize::DEFAULT,
    )
}

#[tokio::test]
async fn empty_mapped_page_commits_exclusive_cursor_and_resume_uses_it() {
    let store = Arc::new(Store::default());
    let id = chat_id();
    store.checkpoints.lock().await.insert(
        id,
        ChatCheckpoint {
            history_before_id: Some(MessageId::new(50).unwrap()),
            ..Default::default()
        },
    );
    let gateway = Arc::new(Gateway {
        pages: Mutex::new(VecDeque::from([
            Ok(page(Some(40), false)),
            Ok(page(None, true)),
        ])),
        boundaries: Mutex::new(vec![]),
        chats: Mutex::new(vec![]),
        fetch_started: tokio::sync::Notify::new(),
    });
    let engine = engine(&store, gateway.clone());
    let cancel = CancellationToken::default();
    engine.sync_chat("job", id, &cancel).await.unwrap();
    let boundaries = gateway.boundaries.lock().await;
    assert_eq!(boundaries[0].before_message_id.unwrap().get(), 50);
    assert_eq!(boundaries[1].before_message_id.unwrap().get(), 40);
    let saved = store.checkpoints.lock().await.get(&id).unwrap().clone();
    assert!(saved.history_complete);
    assert_eq!(saved.history_before_id, None);
    assert_eq!(store.progress.lock().await.len(), 2);
}

#[tokio::test]
async fn flood_wait_cancellation_returns_without_retrying_request() {
    let store = Arc::new(Store::default());
    let gateway = Arc::new(Gateway {
        pages: Mutex::new(VecDeque::from([Err(TelegramError::FloodWait {
            retry_after_seconds: 60,
        })])),
        boundaries: Mutex::new(vec![]),
        chats: Mutex::new(vec![]),
        fetch_started: tokio::sync::Notify::new(),
    });
    let engine = Arc::new(engine(&store, gateway.clone()));
    let token = CancellationToken::default();
    let task_token = token.clone();
    let task = tokio::spawn(async move { engine.sync_chat("job", chat_id(), &task_token).await });
    while gateway.boundaries.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }
    token.cancel();
    assert!(task.await.unwrap().is_err());
    assert_eq!(gateway.boundaries.lock().await.len(), 1);
}

#[tokio::test]
async fn coordinator_rejects_unknown_and_duplicate_scope_and_drains_queue_on_shutdown() {
    let store = Arc::new(Store::default());
    let one = chat(1);
    let two = chat(2);
    let three = chat(3);
    store
        .save_refresh(vec![one.clone(), two.clone(), three.clone()])
        .await
        .unwrap();
    let (coordinator, writer, gateway) = make_coordinator(
        &store,
        vec![],
        vec![Err(TelegramError::FloodWait {
            retry_after_seconds: 60,
        })],
        1,
    );

    assert!(matches!(
        coordinator
            .submit(SyncScope::Chat(
                ChatId::from_telegram(ChatKind::Private, 99).unwrap()
            ))
            .await,
        Err(telegram_message_archive::application::ApplicationError::NotFound)
    ));
    assert!(store.jobs.lock().await.is_empty());
    let first = coordinator.submit(SyncScope::Chat(one.id)).await.unwrap();
    gateway.fetch_started.notified().await;
    assert!(matches!(
        coordinator.submit(SyncScope::Chat(one.id)).await,
        Err(telegram_message_archive::application::ApplicationError::Conflict)
    ));
    let queued = coordinator.submit(SyncScope::Chat(two.id)).await.unwrap();
    assert!(matches!(
        coordinator.submit(SyncScope::Chat(three.id)).await,
        Err(telegram_message_archive::application::ApplicationError::Busy)
    ));
    coordinator.shutdown().await.unwrap();
    assert_eq!(
        coordinator.get_job(&first.id).await.unwrap().state,
        SyncJobState::Interrupted
    );
    assert_eq!(
        coordinator.get_job(&queued.id).await.unwrap().state,
        SyncJobState::Interrupted
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), writer)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    drop(coordinator);
}

#[tokio::test]
async fn sync_all_keeps_partial_chat_outcomes_and_does_not_rewrite_creation_time() {
    let store = Arc::new(Store::default());
    let one = chat(11);
    let two = chat(12);
    let (coordinator, writer, _) = make_coordinator(
        &store,
        vec![one.clone(), two.clone()],
        vec![Err(TelegramError::Unauthorized), Ok(page(None, true))],
        1,
    );
    let job = coordinator.submit(SyncScope::All).await.unwrap();
    let finished = coordinator.wait_job(&job.id).await.unwrap();
    assert_eq!(finished.state, SyncJobState::Failed);
    assert_eq!(finished.created_at, job.created_at);
    let outcomes = store.list_chat_progress(&job.id).await.unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(
        outcomes
            .iter()
            .find(|item| item.chat_id == one.id)
            .unwrap()
            .state,
        SyncJobState::Failed
    );
    assert_eq!(
        outcomes
            .iter()
            .find(|item| item.chat_id == two.id)
            .unwrap()
            .state,
        SyncJobState::Succeeded
    );
    coordinator.shutdown().await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), writer)
            .await
            .unwrap()
            .unwrap()
            .is_ok()
    );
    drop(coordinator);
}

#[tokio::test]
async fn writer_and_terminal_repository_failures_reach_waiter_and_supervisor() {
    let store = Arc::new(Store::default());
    let one = chat(21);
    store.save_refresh(vec![one.clone()]).await.unwrap();
    store.fail_writer.store(true, Ordering::SeqCst);
    let (coordinator, writer, _) = make_coordinator(&store, vec![], vec![Ok(page(None, true))], 1);
    let job = coordinator.submit(SyncScope::Chat(one.id)).await.unwrap();
    assert!(coordinator.wait_job(&job.id).await.is_err());
    assert!(coordinator.shutdown().await.is_err());
    drop(coordinator);
    assert!(writer.await.unwrap().is_err());

    let store = Arc::new(Store::default());
    let one = chat(22);
    store.save_refresh(vec![one.clone()]).await.unwrap();
    store.fail_terminal_job.store(true, Ordering::SeqCst);
    let (coordinator, writer, _) = make_coordinator(&store, vec![], vec![Ok(page(None, true))], 1);
    let job = coordinator.submit(SyncScope::Chat(one.id)).await.unwrap();
    assert!(coordinator.wait_job(&job.id).await.is_err());
    assert_eq!(
        coordinator.get_job(&job.id).await.unwrap().state,
        SyncJobState::Interrupted
    );
    assert!(coordinator.shutdown().await.is_err());
    drop(coordinator);
    assert!(writer.await.unwrap().is_ok());
}

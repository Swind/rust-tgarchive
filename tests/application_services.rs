use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tgarchive::{
    application::services::{Application, ComponentState, ComponentStatus, IngestMessageEvent},
    application::{
        ApplicationError, ArchiveWriter, ChatCheckpoint, ChatRepository, HistoryBoundary,
        HistoryPage, IngestBatch, ListMessagesQuery, MessagePage, MessageRepository, MessageSource,
        PageSize, RepositoryError, SearchMessagesQuery, SyncChatProgress, SyncJob, SyncJobState,
        SyncRepository, TelegramError, TelegramGateway, TimeRange,
    },
    domain::{Attachment, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId},
};
use tokio::sync::Mutex;

#[derive(Default)]
struct State {
    chats: HashMap<ChatId, Chat>,
    messages: HashMap<(ChatId, MessageId), Message>,
    jobs: Vec<SyncJob>,
    writes: Vec<IngestBatch>,
    last_list: Option<ListMessagesQuery>,
    last_search: Option<SearchMessagesQuery>,
    list_calls: usize,
    search_calls: usize,
    chat_get_calls: usize,
    writer_error: Option<String>,
    message_list_error: Option<String>,
    telegram_error: Option<String>,
    refresh_error: Option<String>,
    sync_error: Option<String>,
}

#[derive(Clone, Default)]
struct Fakes(Arc<Mutex<State>>);

#[async_trait]
impl MessageRepository for Fakes {
    async fn get(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        _include_deleted: bool,
    ) -> Result<Option<tgarchive::application::MessageView>, RepositoryError> {
        Ok(self
            .0
            .lock()
            .await
            .messages
            .get(&(chat_id, message_id))
            .cloned()
            .map(Into::into))
    }

    async fn list(&self, query: ListMessagesQuery) -> Result<MessagePage, RepositoryError> {
        let mut state = self.0.lock().await;
        state.list_calls += 1;
        state.last_list = Some(query);
        if let Some(error) = &state.message_list_error {
            return Err(RepositoryError::Unavailable(error.clone()));
        }
        Ok(empty_page())
    }

    async fn search(&self, query: SearchMessagesQuery) -> Result<MessagePage, RepositoryError> {
        let mut state = self.0.lock().await;
        state.search_calls += 1;
        state.last_search = Some(query);
        Ok(empty_page())
    }
}

#[async_trait]
impl ChatRepository for Fakes {
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError> {
        let mut state = self.0.lock().await;
        state.chat_get_calls += 1;
        Ok(state.chats.get(&id).cloned())
    }

    async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
        Ok(self.0.lock().await.chats.values().cloned().collect())
    }

    async fn save_refresh(&self, chats: Vec<Chat>) -> Result<(), RepositoryError> {
        let mut state = self.0.lock().await;
        if let Some(error) = &state.refresh_error {
            return Err(RepositoryError::Unavailable(error.clone()));
        }
        state
            .chats
            .extend(chats.into_iter().map(|chat| (chat.id, chat)));
        Ok(())
    }

    async fn set_tracked(
        &self,
        id: ChatId,
        tracked: bool,
    ) -> Result<Option<Chat>, RepositoryError> {
        let mut state = self.0.lock().await;
        Ok(state.chats.get_mut(&id).map(|chat| {
            chat.tracked = tracked;
            chat.clone()
        }))
    }
}

#[async_trait]
impl ArchiveWriter for Fakes {
    async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
        let mut state = self.0.lock().await;
        state.writes.push(batch);
        if let Some(error) = &state.writer_error {
            return Err(RepositoryError::Unavailable(error.clone()));
        }
        Ok(())
    }
}

#[async_trait]
impl TelegramGateway for Fakes {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        let state = self.0.lock().await;
        if let Some(error) = &state.telegram_error {
            return Err(TelegramError::Unavailable(error.clone()));
        }
        Ok(vec![chat()])
    }

    async fn fetch_history(
        &self,
        _: ChatId,
        _: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
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

#[async_trait]
impl SyncRepository for Fakes {
    async fn get_checkpoint(&self, _: ChatId) -> Result<Option<ChatCheckpoint>, RepositoryError> {
        Ok(None)
    }

    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError> {
        self.0.lock().await.jobs.push(job);
        Ok(())
    }

    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError> {
        Ok(self
            .0
            .lock()
            .await
            .jobs
            .iter()
            .find(|job| job.id == id)
            .cloned())
    }

    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        let state = self.0.lock().await;
        if let Some(error) = &state.sync_error {
            return Err(RepositoryError::Unavailable(error.clone()));
        }
        Ok(state.jobs.clone())
    }

    async fn list_chat_progress(&self, _: &str) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        Ok(vec![])
    }

    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

fn app(fakes: &Fakes, gateway: Option<Arc<dyn TelegramGateway>>) -> Application {
    Application::new(
        Arc::new(fakes.clone()),
        Arc::new(fakes.clone()),
        Arc::new(fakes.clone()),
        Arc::new(fakes.clone()),
        gateway,
        ComponentStatus::disabled(),
    )
}

fn empty_page() -> MessagePage {
    MessagePage {
        items: vec![],
        has_more: false,
        next_cursor: None,
    }
}

fn chat() -> Chat {
    Chat {
        id: ChatId::from_telegram(ChatKind::Private, 5).unwrap(),
        kind: ChatKind::Private,
        title: Some("test chat".into()),
        username: None,
        tracked: false,
    }
}

fn message() -> Message {
    Message {
        id: MessageId::new(10).unwrap(),
        chat_id: chat().id,
        sender_id: None,
        timestamp: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        edited_at: None,
        collected_at: DateTime::<Utc>::from_timestamp(1_700_000_001, 0).unwrap(),
        text: Some("hello".into()),
        reply_to: None,
        attachments: vec![Attachment {
            kind: tgarchive::domain::AttachmentKind::Photo,
            telegram_file_id: None,
            mime_type: None,
            file_name: None,
            size: None,
        }],
    }
}

fn list_query() -> ListMessagesQuery {
    ListMessagesQuery {
        filters: tgarchive::application::MessageFilters {
            chat_id: None,
            sender_id: None,
            time_range: TimeRange::new(None, None).unwrap(),
            include_deleted: false,
        },
        before: None,
        after: None,
        page_size: PageSize::DEFAULT,
    }
}

fn search_query(text: &str) -> SearchMessagesQuery {
    SearchMessagesQuery {
        text: text.into(),
        filters: list_query().filters,
        before: None,
        after: None,
        page_size: PageSize::DEFAULT,
    }
}

#[tokio::test]
async fn ingestion_uses_one_batch_path_and_propagates_writer_failure() {
    let fakes = Fakes::default();
    let app = app(&fakes, None);
    let original = message();
    for event in [
        MessageEvent::Created(original.clone()),
        MessageEvent::Updated(original.clone()),
        MessageEvent::Deleted {
            chat_id: original.chat_id,
            message_id: original.id,
            deleted_at: original.collected_at,
        },
    ] {
        app.ingest_event(IngestMessageEvent {
            event,
            source: MessageSource::History,
            chats: vec![chat()],
            senders: vec![tgarchive::domain::Sender {
                id: tgarchive::domain::SenderId::from_telegram(
                    tgarchive::domain::SenderKind::User,
                    77,
                )
                .unwrap(),
                kind: tgarchive::domain::SenderKind::User,
                display_name: Some("Sender".into()),
                username: None,
            }],
            checkpoint: Some((
                chat().id,
                ChatCheckpoint {
                    history_before_id: Some(MessageId::new(9).unwrap()),
                    history_complete: false,
                    catchup_after_id: Some(MessageId::new(11).unwrap()),
                },
            )),
            job_progress: Some(SyncChatProgress {
                job_id: "job-1".into(),
                chat_id: chat().id,
                state: SyncJobState::Running,
                committed_messages: 3,
                summary_error: None,
            }),
        })
        .await
        .unwrap();
    }

    let state = fakes.0.lock().await;
    assert_eq!(state.writes.len(), 3);
    assert!(state.writes.iter().all(|batch| batch.records.len() == 1));
    assert!(state.writes.iter().all(|batch| batch.chats == vec![chat()]));
    assert!(state.writes.iter().all(|batch| batch.senders.len() == 1));
    assert!(state.writes.iter().all(|batch| batch.checkpoint.is_some()));
    assert!(
        state
            .writes
            .iter()
            .all(|batch| batch.job_progress.is_some())
    );
    assert!(
        state
            .writes
            .iter()
            .all(|batch| batch.records[0].source == MessageSource::History)
    );
    assert!(
        state
            .writes
            .iter()
            .all(|batch| batch.senders[0].display_name.as_deref() == Some("Sender"))
    );
    assert!(state.writes.iter().all(|batch| {
        batch.checkpoint.as_ref()
            == Some(&(
                chat().id,
                ChatCheckpoint {
                    history_before_id: Some(MessageId::new(9).unwrap()),
                    history_complete: false,
                    catchup_after_id: Some(MessageId::new(11).unwrap()),
                },
            ))
    }));
    assert!(state.writes.iter().all(|batch| {
        batch.job_progress.as_ref()
            == Some(&SyncChatProgress {
                job_id: "job-1".into(),
                chat_id: chat().id,
                state: SyncJobState::Running,
                committed_messages: 3,
                summary_error: None,
            })
    }));
    drop(state);

    fakes.0.lock().await.writer_error = Some("disk full".into());
    let error = app
        .ingest_event(IngestMessageEvent {
            event: MessageEvent::Created(original),
            source: MessageSource::Realtime,
            chats: vec![],
            senders: vec![],
            checkpoint: None,
            job_progress: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(error, ApplicationError::RepositoryUnavailable(message) if message == "disk full")
    );
}

#[tokio::test]
async fn message_queries_validate_before_repository_and_report_missing_entities() {
    let fakes = Fakes::default();
    let app = app(&fakes, None);
    let mut invalid = list_query();
    invalid.before = Some(cursor());
    invalid.after = Some(cursor());
    assert!(matches!(
        app.list_messages(invalid).await,
        Err(ApplicationError::Validation(_))
    ));
    assert!(matches!(
        app.search_messages(search_query("  ")).await,
        Err(ApplicationError::Validation(_))
    ));
    {
        let state = fakes.0.lock().await;
        assert_eq!(state.list_calls, 0);
        assert_eq!(state.search_calls, 0);
        assert_eq!(state.chat_get_calls, 0);
    }

    assert!(matches!(
        app.get_message(chat().id, message().id, false).await,
        Err(ApplicationError::NotFound)
    ));
    assert!(matches!(
        app.get_chat(chat().id).await,
        Err(ApplicationError::NotFound)
    ));
    let mut filtered = list_query();
    filtered.filters.chat_id = Some(chat().id);
    assert!(matches!(
        app.list_messages(filtered).await,
        Err(ApplicationError::NotFound)
    ));

    {
        let mut state = fakes.0.lock().await;
        state.chats.insert(chat().id, chat());
        state.messages.insert((chat().id, message().id), message());
    }
    assert_eq!(
        app.get_message(chat().id, message().id, false)
            .await
            .unwrap(),
        message().into()
    );
    assert_eq!(app.get_chat(chat().id).await.unwrap(), chat());

    let mut filtered_list = list_query();
    filtered_list.filters.chat_id = Some(chat().id);
    filtered_list.before = Some(cursor());
    filtered_list.filters.sender_id = Some(
        tgarchive::domain::SenderId::from_telegram(tgarchive::domain::SenderKind::User, 77)
            .unwrap(),
    );
    app.list_messages(filtered_list.clone()).await.unwrap();
    let mut filtered_search = search_query("exact search terms");
    filtered_search.filters.chat_id = Some(chat().id);
    filtered_search.after = Some(cursor());
    app.search_messages(filtered_search.clone()).await.unwrap();
    {
        let state = fakes.0.lock().await;
        assert_eq!(state.last_list.as_ref(), Some(&filtered_list));
        assert_eq!(state.last_search.as_ref(), Some(&filtered_search));
        assert_eq!(state.list_calls, 1);
        assert_eq!(state.search_calls, 1);
    }

    fakes.0.lock().await.message_list_error = Some("read failed".into());
    assert!(
        matches!(app.list_messages(list_query()).await, Err(ApplicationError::RepositoryUnavailable(message)) if message == "read failed")
    );
}

#[tokio::test]
async fn refresh_and_status_report_failures_and_disabled_collector() {
    let fakes = Fakes::default();
    let query_only = app(&fakes, None);
    assert!(matches!(
        query_only.refresh_chats().await,
        Err(ApplicationError::TelegramUnavailable(_))
    ));
    let status = query_only.sync_status().await.unwrap();
    assert_eq!(status.collector.state, ComponentState::Disabled);

    let with_gateway = app(&fakes, Some(Arc::new(fakes.clone())));
    assert_eq!(with_gateway.refresh_chats().await.unwrap(), vec![chat()]);
    assert_eq!(with_gateway.get_chat(chat().id).await.unwrap(), chat());

    let job = SyncJob {
        id: "sync-1".into(),
        scope: tgarchive::application::SyncScope::Chat(chat().id),
        state: SyncJobState::Succeeded,
        created_at: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        started_at: None,
        completed_at: None,
        summary_error: None,
    };
    fakes.0.lock().await.jobs.push(job.clone());
    assert_eq!(
        with_gateway.sync_status().await.unwrap().sync_jobs,
        vec![job]
    );

    fakes.0.lock().await.telegram_error = Some("offline".into());
    assert!(
        matches!(with_gateway.refresh_chats().await, Err(ApplicationError::TelegramUnavailable(message)) if message == "offline")
    );

    {
        let mut state = fakes.0.lock().await;
        state.telegram_error = None;
        state.refresh_error = Some("write failed".into());
    }
    assert!(
        matches!(with_gateway.refresh_chats().await, Err(ApplicationError::RepositoryUnavailable(message)) if message == "write failed")
    );

    fakes.0.lock().await.sync_error = Some("job storage unavailable".into());
    assert!(
        matches!(with_gateway.sync_status().await, Err(ApplicationError::RepositoryUnavailable(message)) if message == "job storage unavailable")
    );
}

fn cursor() -> tgarchive::application::MessageCursor {
    tgarchive::application::MessageCursor {
        timestamp: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        chat_id: chat().id,
        message_id: MessageId::new(2).unwrap(),
    }
}

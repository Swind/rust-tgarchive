use std::sync::Arc;

use crate::{
    application::{
        ApplicationError, ArchiveWriter, ChatCheckpoint, ChatRepository, IngestBatch,
        ListMessagesQuery, MessagePage, MessageRepository, MessageSource, SearchMessagesQuery,
        SyncJob, SyncRepository, TelegramGateway,
    },
    domain::{Chat, ChatId, Message, MessageEvent, MessageId, Sender},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentState {
    Disabled,
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentStatus {
    pub state: ComponentState,
    pub detail: Option<String>,
}

impl ComponentStatus {
    pub const fn disabled() -> Self {
        Self {
            state: ComponentState::Disabled,
            detail: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationStatus {
    pub collector: ComponentStatus,
    pub sync_jobs: Vec<SyncJob>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestMessageEvent {
    pub event: MessageEvent,
    pub source: MessageSource,
    pub chats: Vec<Chat>,
    pub senders: Vec<Sender>,
    pub checkpoint: Option<(ChatId, ChatCheckpoint)>,
    pub job_progress: Option<crate::application::SyncChatProgress>,
}

pub struct IngestionService {
    writer: Arc<dyn ArchiveWriter>,
}

impl IngestionService {
    pub fn new(writer: Arc<dyn ArchiveWriter>) -> Self {
        Self { writer }
    }

    pub async fn ingest_event(&self, input: IngestMessageEvent) -> Result<(), ApplicationError> {
        self.ingest_batch(IngestBatch {
            chats: input.chats,
            senders: input.senders,
            records: vec![crate::application::IngestRecord {
                event: input.event,
                source: input.source,
            }],
            checkpoint: input.checkpoint,
            job_progress: input.job_progress,
            ..IngestBatch::default()
        })
        .await
    }

    pub async fn ingest_batch(&self, batch: IngestBatch) -> Result<(), ApplicationError> {
        self.writer.write_batch(batch).await.map_err(Into::into)
    }
}

pub struct MessageService {
    messages: Arc<dyn MessageRepository>,
    chats: Arc<dyn ChatRepository>,
}

impl MessageService {
    pub fn new(messages: Arc<dyn MessageRepository>, chats: Arc<dyn ChatRepository>) -> Self {
        Self { messages, chats }
    }

    pub async fn get(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<Message, ApplicationError> {
        self.messages
            .get(chat_id, message_id)
            .await
            .map_err(ApplicationError::from)?
            .ok_or(ApplicationError::NotFound)
    }

    pub async fn list(&self, query: ListMessagesQuery) -> Result<MessagePage, ApplicationError> {
        query.validate()?;
        self.ensure_chat_exists(query.filters.chat_id).await?;
        self.messages
            .list(query)
            .await
            .map_err(ApplicationError::from)
    }

    pub async fn search(
        &self,
        query: SearchMessagesQuery,
    ) -> Result<MessagePage, ApplicationError> {
        query.validate()?;
        self.ensure_chat_exists(query.filters.chat_id).await?;
        self.messages
            .search(query)
            .await
            .map_err(ApplicationError::from)
    }

    async fn ensure_chat_exists(&self, chat_id: Option<ChatId>) -> Result<(), ApplicationError> {
        if let Some(chat_id) = chat_id
            && self
                .chats
                .get(chat_id)
                .await
                .map_err(ApplicationError::from)?
                .is_none()
        {
            return Err(ApplicationError::NotFound);
        }
        Ok(())
    }
}

pub struct ChatService {
    chats: Arc<dyn ChatRepository>,
}

impl ChatService {
    pub fn new(chats: Arc<dyn ChatRepository>) -> Self {
        Self { chats }
    }

    pub async fn get(&self, id: ChatId) -> Result<Chat, ApplicationError> {
        self.chats
            .get(id)
            .await
            .map_err(ApplicationError::from)?
            .ok_or(ApplicationError::NotFound)
    }

    pub async fn list(&self) -> Result<Vec<Chat>, ApplicationError> {
        self.chats.list().await.map_err(ApplicationError::from)
    }
}

pub struct RefreshChatsService {
    gateway: Option<Arc<dyn TelegramGateway>>,
    chats: Arc<dyn ChatRepository>,
}

impl RefreshChatsService {
    pub fn new(gateway: Option<Arc<dyn TelegramGateway>>, chats: Arc<dyn ChatRepository>) -> Self {
        Self { gateway, chats }
    }

    pub async fn refresh(&self) -> Result<Vec<Chat>, ApplicationError> {
        let gateway = self.gateway.as_ref().ok_or_else(|| {
            ApplicationError::TelegramUnavailable("Telegram gateway is not configured".into())
        })?;
        let chats = gateway.list_chats().await.map_err(ApplicationError::from)?;
        self.chats
            .save_refresh(chats.clone())
            .await
            .map_err(ApplicationError::from)?;
        Ok(chats)
    }
}

pub struct SyncStatusService {
    sync: Arc<dyn SyncRepository>,
    collector: ComponentStatus,
}

impl SyncStatusService {
    pub fn new(sync: Arc<dyn SyncRepository>, collector: ComponentStatus) -> Self {
        Self { sync, collector }
    }

    pub async fn get(&self) -> Result<ApplicationStatus, ApplicationError> {
        let sync_jobs = self
            .sync
            .list_jobs()
            .await
            .map_err(ApplicationError::from)?;
        Ok(ApplicationStatus {
            collector: self.collector.clone(),
            sync_jobs,
        })
    }
}

pub struct Application {
    ingestion: IngestionService,
    messages: MessageService,
    chats: ChatService,
    refresh_chats: RefreshChatsService,
    sync_status: SyncStatusService,
}

impl Application {
    pub fn new(
        writer: Arc<dyn ArchiveWriter>,
        message_repository: Arc<dyn MessageRepository>,
        chat_repository: Arc<dyn ChatRepository>,
        sync_repository: Arc<dyn SyncRepository>,
        telegram_gateway: Option<Arc<dyn TelegramGateway>>,
        collector_status: ComponentStatus,
    ) -> Self {
        Self {
            ingestion: IngestionService::new(writer),
            messages: MessageService::new(message_repository, chat_repository.clone()),
            chats: ChatService::new(chat_repository.clone()),
            refresh_chats: RefreshChatsService::new(telegram_gateway, chat_repository),
            sync_status: SyncStatusService::new(sync_repository, collector_status),
        }
    }

    pub async fn ingest_event(&self, input: IngestMessageEvent) -> Result<(), ApplicationError> {
        self.ingestion.ingest_event(input).await
    }

    pub async fn ingest_batch(&self, batch: IngestBatch) -> Result<(), ApplicationError> {
        self.ingestion.ingest_batch(batch).await
    }

    pub async fn get_message(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<Message, ApplicationError> {
        self.messages.get(chat_id, message_id).await
    }

    pub async fn list_messages(
        &self,
        query: ListMessagesQuery,
    ) -> Result<MessagePage, ApplicationError> {
        self.messages.list(query).await
    }

    pub async fn search_messages(
        &self,
        query: SearchMessagesQuery,
    ) -> Result<MessagePage, ApplicationError> {
        self.messages.search(query).await
    }

    pub async fn get_chat(&self, id: ChatId) -> Result<Chat, ApplicationError> {
        self.chats.get(id).await
    }

    pub async fn list_chats(&self) -> Result<Vec<Chat>, ApplicationError> {
        self.chats.list().await
    }

    pub async fn refresh_chats(&self) -> Result<Vec<Chat>, ApplicationError> {
        self.refresh_chats.refresh().await
    }

    pub async fn sync_status(&self) -> Result<ApplicationStatus, ApplicationError> {
        self.sync_status.get().await
    }
}

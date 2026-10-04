use std::sync::Arc;

use crate::{
    application::{
        ApplicationError, ArchiveWriter, ChatCheckpoint, ChatRepository, ChatSort, ChatSummary,
        IngestBatch, ListMessagesQuery, MessageContext, MessageContextQuery, MessagePage,
        MessageRepository, MessageSource, MessageView, PageSize, SearchMessagesQuery,
        SenderSummary, SyncJob, SyncRepository, TelegramGateway,
        pacer::{RateLimitStatus, RatePacer},
    },
    domain::{Chat, ChatId, MessageEvent, MessageId, Sender},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentState {
    Disabled,
    Starting,
    CatchingUp,
    Running,
    Reconnecting,
    Stopped,
    Failed,
    Degraded,
    Unavailable,
}

impl ComponentState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Starting => "starting",
            Self::CatchingUp => "catching_up",
            Self::Running => "running",
            Self::Reconnecting => "reconnecting",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }
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

    pub fn new(state: ComponentState, detail: impl Into<Option<String>>) -> Self {
        Self {
            state,
            detail: detail.into(),
        }
    }
}

/// Shared, cheaply clonable collector state: the realtime supervisor writes it and the status
/// service reads it.
#[derive(Debug, Clone)]
pub struct CollectorStatusHandle(Arc<std::sync::Mutex<ComponentStatus>>);

impl CollectorStatusHandle {
    pub fn new(initial: ComponentStatus) -> Self {
        Self(Arc::new(std::sync::Mutex::new(initial)))
    }

    pub fn set(&self, state: ComponentState, detail: impl Into<Option<String>>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = ComponentStatus::new(state, detail);
    }

    pub fn get(&self) -> ComponentStatus {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl From<ComponentStatus> for CollectorStatusHandle {
    fn from(status: ComponentStatus) -> Self {
        Self::new(status)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationStatus {
    pub collector: ComponentStatus,
    pub sync_jobs: Vec<SyncJob>,
    pub unresolved_deletions: u64,
    /// History-request pacing; `None` when this process does not talk to Telegram.
    pub rate_limit: Option<RateLimitStatus>,
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
        include_deleted: bool,
    ) -> Result<MessageView, ApplicationError> {
        self.messages
            .get(chat_id, message_id, include_deleted)
            .await
            .map_err(ApplicationError::from)?
            .ok_or(ApplicationError::NotFound)
    }

    pub async fn context(
        &self,
        query: MessageContextQuery,
    ) -> Result<MessageContext, ApplicationError> {
        query.validate()?;
        self.messages
            .context(query)
            .await
            .map_err(ApplicationError::from)?
            .ok_or(ApplicationError::NotFound)
    }

    pub async fn list_senders(
        &self,
        chat_id: ChatId,
        limit: PageSize,
    ) -> Result<Vec<SenderSummary>, ApplicationError> {
        self.ensure_chat_exists(Some(chat_id)).await?;
        self.messages
            .list_senders(chat_id, limit)
            .await
            .map_err(ApplicationError::from)
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

    pub async fn list(&self, tracked_only: bool) -> Result<Vec<Chat>, ApplicationError> {
        let mut chats = self.chats.list().await.map_err(ApplicationError::from)?;
        if tracked_only {
            chats.retain(|chat| chat.tracked);
        }
        Ok(chats)
    }

    pub async fn get_summary(&self, id: ChatId) -> Result<ChatSummary, ApplicationError> {
        self.chats
            .get_with_stats(id)
            .await
            .map_err(ApplicationError::from)?
            .ok_or(ApplicationError::NotFound)
    }

    pub async fn list_summaries(
        &self,
        tracked_only: bool,
        sort: ChatSort,
    ) -> Result<Vec<ChatSummary>, ApplicationError> {
        let mut chats = self
            .chats
            .list_with_stats(sort)
            .await
            .map_err(ApplicationError::from)?;
        if tracked_only {
            chats.retain(|chat| chat.tracked);
        }
        Ok(chats)
    }

    /// Idempotent. Unknown chats are `NotFound`; untracking keeps all stored messages.
    pub async fn set_tracked(&self, id: ChatId, tracked: bool) -> Result<Chat, ApplicationError> {
        self.chats
            .set_tracked(id, tracked)
            .await
            .map_err(ApplicationError::from)?
            .ok_or(ApplicationError::NotFound)
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
    collector: CollectorStatusHandle,
    pacer: Option<Arc<RatePacer>>,
}

impl SyncStatusService {
    pub fn new(sync: Arc<dyn SyncRepository>, collector: CollectorStatusHandle) -> Self {
        Self {
            sync,
            collector,
            pacer: None,
        }
    }

    pub async fn get(&self) -> Result<ApplicationStatus, ApplicationError> {
        let sync_jobs = self
            .sync
            .list_jobs()
            .await
            .map_err(ApplicationError::from)?;
        let unresolved_deletions = self
            .sync
            .unresolved_deletions()
            .await
            .map_err(ApplicationError::from)?;
        Ok(ApplicationStatus {
            collector: self.collector.get(),
            sync_jobs,
            unresolved_deletions,
            rate_limit: self.pacer.as_ref().map(|pacer| pacer.status()),
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
        collector_status: impl Into<CollectorStatusHandle>,
    ) -> Self {
        Self {
            ingestion: IngestionService::new(writer),
            messages: MessageService::new(message_repository, chat_repository.clone()),
            chats: ChatService::new(chat_repository.clone()),
            refresh_chats: RefreshChatsService::new(telegram_gateway, chat_repository),
            sync_status: SyncStatusService::new(sync_repository, collector_status.into()),
        }
    }

    /// Exposes the shared history pacer in `sync_status`.
    pub fn with_rate_limit(mut self, pacer: Arc<RatePacer>) -> Self {
        self.sync_status.pacer = Some(pacer);
        self
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
        include_deleted: bool,
    ) -> Result<MessageView, ApplicationError> {
        self.messages
            .get(chat_id, message_id, include_deleted)
            .await
    }

    pub async fn message_context(
        &self,
        query: MessageContextQuery,
    ) -> Result<MessageContext, ApplicationError> {
        self.messages.context(query).await
    }

    pub async fn chat_senders(
        &self,
        chat_id: ChatId,
        limit: PageSize,
    ) -> Result<Vec<SenderSummary>, ApplicationError> {
        self.messages.list_senders(chat_id, limit).await
    }

    pub async fn get_chat_summary(&self, id: ChatId) -> Result<ChatSummary, ApplicationError> {
        self.chats.get_summary(id).await
    }

    pub async fn list_chat_summaries(
        &self,
        tracked_only: bool,
        sort: ChatSort,
    ) -> Result<Vec<ChatSummary>, ApplicationError> {
        self.chats.list_summaries(tracked_only, sort).await
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

    pub async fn list_chats(&self, tracked_only: bool) -> Result<Vec<Chat>, ApplicationError> {
        self.chats.list(tracked_only).await
    }

    pub async fn track_chat(&self, id: ChatId) -> Result<Chat, ApplicationError> {
        self.chats.set_tracked(id, true).await
    }

    pub async fn untrack_chat(&self, id: ChatId) -> Result<Chat, ApplicationError> {
        self.chats.set_tracked(id, false).await
    }

    pub async fn refresh_chats(&self) -> Result<Vec<Chat>, ApplicationError> {
        self.refresh_chats.refresh().await
    }

    pub async fn sync_status(&self) -> Result<ApplicationStatus, ApplicationError> {
        self.sync_status.get().await
    }
}

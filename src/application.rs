use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{Chat, ChatId, Message, MessageEvent, MessageId, Sender, SenderId};

const MAX_SEARCH_LENGTH: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct PageSize(u16);

impl PageSize {
    pub const DEFAULT: Self = Self(100);
    pub const MAX: u16 = 1000;

    pub fn new(value: u16) -> Result<Self, ValidationError> {
        if (1..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ValidationError::InvalidPageSize)
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for PageSize {
    type Error = ValidationError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<PageSize> for u16 {
    fn from(value: PageSize) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageCursor {
    pub timestamp: DateTime<Utc>,
    pub chat_id: ChatId,
    pub message_id: MessageId,
}

impl MessageCursor {
    pub fn compare_key(&self, other: &Self) -> std::cmp::Ordering {
        (self.timestamp, self.chat_id.get(), self.message_id.get()).cmp(&(
            other.timestamp,
            other.chat_id.get(),
            other.message_id.get(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}

impl TimeRange {
    pub fn new(
        from: Option<DateTime<Utc>>,
        to: Option<DateTime<Utc>>,
    ) -> Result<Self, ValidationError> {
        let range = Self { from, to };
        range.validate()?;
        Ok(range)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if matches!((self.from, self.to), (Some(start), Some(end)) if start >= end) {
            return Err(ValidationError::InvalidTimeRange);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDirection {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFilters {
    pub chat_id: Option<ChatId>,
    pub sender_id: Option<SenderId>,
    pub time_range: TimeRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListMessagesQuery {
    pub filters: MessageFilters,
    pub before: Option<MessageCursor>,
    pub after: Option<MessageCursor>,
    pub page_size: PageSize,
}

impl ListMessagesQuery {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.filters.time_range.validate()?;
        if self.before.is_some() && self.after.is_some() {
            return Err(ValidationError::ConflictingCursors);
        }
        Ok(())
    }

    pub fn direction(&self) -> PageDirection {
        if self.after.is_some() {
            PageDirection::After
        } else {
            PageDirection::Before
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMessagesQuery {
    pub text: String,
    pub filters: MessageFilters,
    pub before: Option<MessageCursor>,
    pub after: Option<MessageCursor>,
    pub page_size: PageSize,
}

impl SearchMessagesQuery {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.filters.time_range.validate()?;
        if self.text.trim().is_empty() {
            return Err(ValidationError::EmptySearch);
        }
        if self.text.len() > MAX_SEARCH_LENGTH {
            return Err(ValidationError::SearchTooLong);
        }
        if self.before.is_some() && self.after.is_some() {
            return Err(ValidationError::ConflictingCursors);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    pub items: Vec<Message>,
    pub has_more: bool,
    pub next_cursor: Option<MessageCursor>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ValidationError {
    #[error("page size must be between 1 and 1000")]
    InvalidPageSize,
    #[error("before and after cursors cannot be used together")]
    ConflictingCursors,
    #[error("time range must have from < to")]
    InvalidTimeRange,
    #[error("search query cannot be empty")]
    EmptySearch,
    #[error("search query exceeds 4096 bytes")]
    SearchTooLong,
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("validation failed: {0}")]
    Validation(#[from] ValidationError),
    #[error("resource not found")]
    NotFound,
    #[error("operation conflicts with current state")]
    Conflict,
    #[error("service is busy")]
    Busy,
    #[error("Telegram is unavailable: {0}")]
    TelegramUnavailable(String),
    #[error("Telegram rate limited for {retry_after_seconds} seconds")]
    TelegramFloodWait { retry_after_seconds: u64 },
    #[error("repository is unavailable: {0}")]
    RepositoryUnavailable(String),
    #[error("internal application error: {0}")]
    Internal(String),
}

impl From<RepositoryError> for ApplicationError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::Unavailable(message) => Self::RepositoryUnavailable(message),
            RepositoryError::InvalidData(message) => Self::Internal(message),
        }
    }
}

impl From<TelegramError> for ApplicationError {
    fn from(error: TelegramError) -> Self {
        match error {
            TelegramError::Unavailable(message) => Self::TelegramUnavailable(message),
            TelegramError::Unauthorized => {
                Self::TelegramUnavailable("Telegram account is not authorized".into())
            }
            TelegramError::FloodWait {
                retry_after_seconds,
            } => Self::TelegramFloodWait {
                retry_after_seconds,
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("storage operation failed: {0}")]
    Unavailable(String),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
}

#[derive(Debug, Error)]
pub enum TelegramError {
    #[error("Telegram request failed: {0}")]
    Unavailable(String),
    #[error("Telegram rate limited for {retry_after_seconds} seconds")]
    FloodWait { retry_after_seconds: u64 },
    #[error("Telegram access is not authorized")]
    Unauthorized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageSource {
    Realtime,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestRecord {
    pub event: MessageEvent,
    pub source: MessageSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChatCheckpoint {
    pub history_before_id: Option<MessageId>,
    pub history_complete: bool,
    pub catchup_after_id: Option<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IngestBatch {
    pub chats: Vec<Chat>,
    pub senders: Vec<Sender>,
    pub records: Vec<IngestRecord>,
    pub checkpoint: Option<(ChatId, ChatCheckpoint)>,
    pub job_progress: Option<SyncChatProgress>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryBoundary {
    pub before_message_id: Option<MessageId>,
    pub after_message_id: Option<MessageId>,
}

impl HistoryBoundary {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.before_message_id.is_some() && self.after_message_id.is_some() {
            return Err(ValidationError::ConflictingCursors);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPage {
    pub chats: Vec<Chat>,
    pub senders: Vec<Sender>,
    pub records: Vec<IngestRecord>,
    pub next_before_message_id: Option<MessageId>,
    pub next_after_message_id: Option<MessageId>,
    pub exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncJobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncJob {
    pub id: String,
    pub scope: SyncScope,
    pub state: SyncJobState,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub summary_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncScope {
    Chat(ChatId),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncChatProgress {
    pub job_id: String,
    pub chat_id: ChatId,
    pub state: SyncJobState,
    pub committed_messages: u64,
    pub summary_error: Option<String>,
}

#[async_trait]
pub trait MessageRepository: Send + Sync {
    async fn get(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
    ) -> Result<Option<Message>, RepositoryError>;
    async fn list(&self, query: ListMessagesQuery) -> Result<MessagePage, RepositoryError>;
    async fn search(&self, query: SearchMessagesQuery) -> Result<MessagePage, RepositoryError>;
}

#[async_trait]
pub trait ChatRepository: Send + Sync {
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError>;
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError>;
    async fn save_refresh(&self, chats: Vec<Chat>) -> Result<(), RepositoryError>;
}

#[async_trait]
pub trait ArchiveWriter: Send + Sync {
    async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError>;
}

#[async_trait]
pub trait TelegramGateway: Send + Sync {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError>;
    async fn fetch_history(
        &self,
        chat_id: ChatId,
        boundary: HistoryBoundary,
        page_size: PageSize,
    ) -> Result<HistoryPage, TelegramError>;
}

#[async_trait]
pub trait SyncRepository: Send + Sync {
    async fn get_checkpoint(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<ChatCheckpoint>, RepositoryError>;
    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError>;
    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError>;
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError>;
    async fn list_chat_progress(
        &self,
        job_id: &str,
    ) -> Result<Vec<SyncChatProgress>, RepositoryError>;
    async fn recover_interrupted(&self) -> Result<(), RepositoryError>;
}

pub mod services;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ChatKind, MessageId};
    use std::sync::Arc;

    fn filters() -> MessageFilters {
        MessageFilters {
            chat_id: None,
            sender_id: None,
            time_range: TimeRange::new(None, None).unwrap(),
        }
    }

    #[test]
    fn page_size_and_query_boundaries_are_validated() {
        assert!(PageSize::new(1).is_ok());
        assert!(PageSize::new(PageSize::MAX).is_ok());
        assert!(PageSize::new(0).is_err());
        assert!(PageSize::new(PageSize::MAX + 1).is_err());
        assert!(serde_json::from_str::<PageSize>("0").is_err());
        assert!(serde_json::from_str::<PageSize>("1001").is_err());
        let cursor = MessageCursor {
            timestamp: DateTime::from_timestamp(10, 0).unwrap(),
            chat_id: ChatId::from_telegram(ChatKind::Private, 3).unwrap(),
            message_id: MessageId::new(2).unwrap(),
        };
        let query = ListMessagesQuery {
            filters: filters(),
            before: Some(cursor.clone()),
            after: Some(cursor),
            page_size: PageSize::DEFAULT,
        };
        assert_eq!(query.validate(), Err(ValidationError::ConflictingCursors));
        assert_eq!(
            TimeRange::new(
                Some(DateTime::from_timestamp(2, 0).unwrap()),
                Some(DateTime::from_timestamp(2, 0).unwrap())
            ),
            Err(ValidationError::InvalidTimeRange)
        );
        let invalid_filters = MessageFilters {
            chat_id: None,
            sender_id: None,
            time_range: TimeRange {
                from: Some(DateTime::from_timestamp(2, 0).unwrap()),
                to: Some(DateTime::from_timestamp(1, 0).unwrap()),
            },
        };
        assert_eq!(
            ListMessagesQuery {
                filters: invalid_filters,
                before: None,
                after: None,
                page_size: PageSize::DEFAULT
            }
            .validate(),
            Err(ValidationError::InvalidTimeRange)
        );
        let too_long = SearchMessagesQuery {
            text: "a".repeat(MAX_SEARCH_LENGTH + 1),
            filters: filters(),
            before: None,
            after: None,
            page_size: PageSize::DEFAULT,
        };
        assert_eq!(too_long.validate(), Err(ValidationError::SearchTooLong));
    }

    #[test]
    fn compound_cursor_orders_same_timestamp_without_collisions() {
        let timestamp = DateTime::from_timestamp(10, 0).unwrap();
        let left = MessageCursor {
            timestamp,
            chat_id: ChatId::from_telegram(ChatKind::Private, 4).unwrap(),
            message_id: MessageId::new(1).unwrap(),
        };
        let right = MessageCursor {
            timestamp,
            chat_id: ChatId::from_telegram(ChatKind::Private, 3).unwrap(),
            message_id: MessageId::new(1).unwrap(),
        };
        assert_eq!(left.compare_key(&right), std::cmp::Ordering::Greater);
    }

    struct FakePorts;

    #[async_trait]
    impl MessageRepository for FakePorts {
        async fn get(&self, _: ChatId, _: MessageId) -> Result<Option<Message>, RepositoryError> {
            Ok(None)
        }
        async fn list(&self, _: ListMessagesQuery) -> Result<MessagePage, RepositoryError> {
            Ok(MessagePage {
                items: vec![],
                has_more: false,
                next_cursor: None,
            })
        }
        async fn search(&self, _: SearchMessagesQuery) -> Result<MessagePage, RepositoryError> {
            Ok(MessagePage {
                items: vec![],
                has_more: false,
                next_cursor: None,
            })
        }
    }

    #[async_trait]
    impl ChatRepository for FakePorts {
        async fn get(&self, _: ChatId) -> Result<Option<Chat>, RepositoryError> {
            Ok(None)
        }
        async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
            Ok(vec![])
        }
        async fn save_refresh(&self, _: Vec<Chat>) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl ArchiveWriter for FakePorts {
        async fn write_batch(&self, _: IngestBatch) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl TelegramGateway for FakePorts {
        async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
            Ok(vec![])
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
    impl SyncRepository for FakePorts {
        async fn get_checkpoint(
            &self,
            _: ChatId,
        ) -> Result<Option<ChatCheckpoint>, RepositoryError> {
            Ok(None)
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
        async fn list_chat_progress(
            &self,
            _: &str,
        ) -> Result<Vec<SyncChatProgress>, RepositoryError> {
            Ok(vec![])
        }
        async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
            Ok(())
        }
    }

    #[test]
    fn fake_implements_all_ports_as_shared_trait_objects() {
        let fake = Arc::new(FakePorts);
        let _: Arc<dyn MessageRepository> = fake.clone();
        let _: Arc<dyn ChatRepository> = fake.clone();
        let _: Arc<dyn ArchiveWriter> = fake.clone();
        let _: Arc<dyn TelegramGateway> = fake.clone();
        let _: Arc<dyn SyncRepository> = fake;
    }
}

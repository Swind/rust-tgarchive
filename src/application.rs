use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    Chat, ChatId, ChatKind, Message, MessageEvent, MessageId, Sender, SenderId, SenderKind,
};

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
    /// Exact channel post signature.
    pub post_author: Option<String>,
    pub time_range: TimeRange,
    /// Also return messages marked deleted (their bodies are retained); tombstones without a
    /// stored message are never returned.
    pub include_deleted: bool,
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

/// Result ordering of a text search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchSort {
    /// BM25 relevance (best first), paged with an offset position. Without a usable full-text
    /// index (single-character queries, index not ready) results are newest first.
    #[default]
    Relevance,
    /// Newest first with keyset cursors.
    Time,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMessagesQuery {
    pub text: String,
    pub filters: MessageFilters,
    /// Keyset cursor; only valid with [`SearchSort::Time`].
    pub before: Option<MessageCursor>,
    /// Keyset cursor; only valid with [`SearchSort::Time`].
    pub after: Option<MessageCursor>,
    pub page_size: PageSize,
    pub sort: SearchSort,
    /// Number of results to skip; only valid with [`SearchSort::Relevance`].
    pub offset: u64,
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
        if !crate::infrastructure::search::is_searchable(&self.text) {
            return Err(ValidationError::EmptySearch);
        }
        if self.before.is_some() && self.after.is_some() {
            return Err(ValidationError::ConflictingCursors);
        }
        let keyset = self.before.is_some() || self.after.is_some();
        if (self.sort == SearchSort::Relevance && keyset)
            || (self.sort == SearchSort::Time && self.offset > 0)
        {
            return Err(ValidationError::ConflictingCursors);
        }
        Ok(())
    }
}

/// Display info of a message sender, resolved from the `senders` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SenderInfo {
    pub id: SenderId,
    pub display_name: Option<String>,
    pub username: Option<String>,
}

/// A stored message plus read-side details (resolved sender, deletion time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageView {
    #[serde(flatten)]
    pub message: Message,
    pub sender: Option<SenderInfo>,
    pub deleted_at: Option<DateTime<Utc>>,
    /// Short excerpt around the first search match; only set by search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

impl MessageView {
    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

impl std::ops::Deref for MessageView {
    type Target = Message;

    fn deref(&self) -> &Message {
        &self.message
    }
}

impl From<Message> for MessageView {
    fn from(message: Message) -> Self {
        Self {
            message,
            sender: None,
            deleted_at: None,
            snippet: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    pub items: Vec<MessageView>,
    pub has_more: bool,
    pub next_cursor: Option<MessageCursor>,
    /// Position of the next page of a relevance-sorted search.
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchIndexState {
    Ready,
    Rebuilding,
    /// Missing or outdated; searches use a slower `LIKE` scan until it is rebuilt.
    Stale,
}

impl SearchIndexState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Rebuilding => "rebuilding",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchIndexStatus {
    /// Index format version recorded in the database (0 = never built).
    pub version: u32,
    pub state: SearchIndexState,
    /// Messages currently present in the full-text index.
    pub indexed: u64,
    /// Messages with text that should be indexed.
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderSummary {
    pub sender: SenderInfo,
    pub message_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SenderSort {
    /// Most messages first.
    #[default]
    Messages,
    /// Most recently active first.
    LastMessage,
    /// Display name ascending (nameless last).
    Name,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderQuery {
    /// Substring of display name/username (NFKC + lowercase), `@username` (exact) or numeric ID.
    pub text: Option<String>,
    pub sort: SenderSort,
    pub limit: PageSize,
    pub offset: u64,
    /// Count deleted messages too.
    pub include_deleted: bool,
}

/// A sender with archive-wide aggregates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderProfile {
    pub id: SenderId,
    pub kind: SenderKind,
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub message_count: u64,
    pub chat_count: u64,
    pub first_message_at: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    /// The Telegram account the archive is bound to.
    pub is_self: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderChatStat {
    pub chat_id: ChatId,
    pub title: Option<String>,
    pub kind: ChatKind,
    pub message_count: u64,
    pub last_message_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderDetail {
    pub profile: SenderProfile,
    pub chats: Vec<SenderChatStat>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderPage {
    pub items: Vec<SenderProfile>,
    pub next_offset: Option<u64>,
}

pub const MAX_CONTEXT_SIZE: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageContextQuery {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub before: u16,
    pub after: u16,
    pub include_deleted: bool,
}

impl MessageContextQuery {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.before > MAX_CONTEXT_SIZE || self.after > MAX_CONTEXT_SIZE {
            return Err(ValidationError::InvalidContextSize);
        }
        Ok(())
    }
}

/// Messages around an anchor; both lists are ascending (oldest first). The cursors continue
/// scrolling through the list endpoint (`before_cursor` -> `before`, `after_cursor` -> `after`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageContext {
    pub anchor: MessageView,
    pub before: Vec<MessageView>,
    pub after: Vec<MessageView>,
    pub has_more_before: bool,
    pub has_more_after: bool,
    pub before_cursor: Option<MessageCursor>,
    pub after_cursor: Option<MessageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatStats {
    /// Non-deleted messages.
    pub message_count: u64,
    pub deleted_count: u64,
    pub first_message_at: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub history_complete: bool,
    pub last_sync_completed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSummary {
    pub chat: Chat,
    pub stats: ChatStats,
}

impl std::ops::Deref for ChatSummary {
    type Target = Chat;

    fn deref(&self) -> &Chat {
        &self.chat
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatSort {
    /// By chat ID.
    #[default]
    Default,
    Title,
    LastMessage,
    MessageCount,
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
    #[error("context size must be between 0 and 100")]
    InvalidContextSize,
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("validation failed: {0}")]
    Validation(#[from] ValidationError),
    #[error("resource not found")]
    NotFound,
    #[error("chat is not tracked; track it first (`chats track <CHAT_ID>`)")]
    NotTracked,
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
    /// A `--refetch` walk: existing rows only get NULL metadata filled (sender, post author,
    /// forward origin, attachment details); text, versions and deletions are never touched.
    Refetch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestRecord {
    pub event: MessageEvent,
    pub source: MessageSource,
}

/// Progress of a resumable `--refetch` walk (newest to oldest), independent of [`ChatCheckpoint`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RefetchCheckpoint {
    /// A walk was started and has not reached the start of the chat yet.
    pub active: bool,
    pub before_id: Option<MessageId>,
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
    /// Account-wide deletions whose Telegram updates omit the originating peer.
    /// The archive must already be bound to the matching Telegram account.
    pub account_deletions: Vec<AccountDeletion>,
    pub checkpoint: Option<(ChatId, ChatCheckpoint)>,
    pub refetch_checkpoint: Option<(ChatId, RefetchCheckpoint)>,
    pub job_progress: Option<SyncChatProgress>,
    /// Sanitized reason a chat could not be synced; stored as `last_error` until the next
    /// checkpoint commit for that chat clears it.
    pub chat_error: Option<(ChatId, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDeletion {
    pub message_id: MessageId,
    pub deleted_at: DateTime<Utc>,
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
    /// Stopped because Telegram asked for a longer wait than allowed; progress is kept and a
    /// later sync resumes from the checkpoint.
    RateLimited,
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
        include_deleted: bool,
    ) -> Result<Option<MessageView>, RepositoryError>;
    async fn list(&self, query: ListMessagesQuery) -> Result<MessagePage, RepositoryError>;
    async fn search(&self, query: SearchMessagesQuery) -> Result<MessagePage, RepositoryError>;

    /// State of the full-text index. Default: ready and empty.
    async fn search_index_status(&self) -> Result<SearchIndexStatus, RepositoryError> {
        Ok(SearchIndexStatus {
            version: 0,
            state: SearchIndexState::Ready,
            indexed: 0,
            total: 0,
        })
    }

    /// Senders with messages in the chat, most active first. Default: none.
    async fn list_senders(
        &self,
        _chat_id: ChatId,
        _limit: PageSize,
    ) -> Result<Vec<SenderSummary>, RepositoryError> {
        Ok(Vec::new())
    }

    /// The Telegram user the archive is bound to. Default: unbound.
    async fn bound_account(&self) -> Result<Option<SenderId>, RepositoryError> {
        Ok(None)
    }

    /// Senders with archive-wide aggregates. Default: none.
    async fn search_senders(&self, _query: SenderQuery) -> Result<SenderPage, RepositoryError> {
        Ok(SenderPage {
            items: Vec::new(),
            next_offset: None,
        })
    }

    /// `None` when nothing is known about the sender. Default: none.
    async fn sender_detail(
        &self,
        _id: SenderId,
        _include_deleted: bool,
    ) -> Result<Option<SenderDetail>, RepositoryError> {
        Ok(None)
    }

    /// `None` when the anchor does not exist (or is deleted and `include_deleted` is false).
    async fn context(
        &self,
        query: MessageContextQuery,
    ) -> Result<Option<MessageContext>, RepositoryError> {
        let Some(anchor) = self
            .get(query.chat_id, query.message_id, query.include_deleted)
            .await?
        else {
            return Ok(None);
        };
        let cursor = MessageCursor {
            timestamp: anchor.timestamp,
            chat_id: query.chat_id,
            message_id: query.message_id,
        };
        let filters = MessageFilters {
            chat_id: Some(query.chat_id),
            sender_id: None,
            post_author: None,
            time_range: TimeRange {
                from: None,
                to: None,
            },
            include_deleted: query.include_deleted,
        };
        let empty = || MessagePage {
            items: Vec::new(),
            has_more: false,
            next_cursor: None,
            next_offset: None,
        };
        let page_size = |count: u16| {
            PageSize::new(count).map_err(|error| RepositoryError::InvalidData(error.to_string()))
        };
        let mut older = empty();
        if query.before > 0 {
            older = self
                .list(ListMessagesQuery {
                    filters: filters.clone(),
                    before: Some(cursor.clone()),
                    after: None,
                    page_size: page_size(query.before)?,
                })
                .await?;
            older.items.reverse();
        }
        let mut newer = empty();
        if query.after > 0 {
            newer = self
                .list(ListMessagesQuery {
                    filters,
                    before: None,
                    after: Some(cursor),
                    page_size: page_size(query.after)?,
                })
                .await?;
            // `after` pages are newest-first like every list page.
            newer.items.reverse();
        }
        Ok(Some(MessageContext {
            anchor,
            before: older.items,
            after: newer.items,
            has_more_before: older.has_more,
            has_more_after: newer.has_more,
            before_cursor: older.next_cursor,
            after_cursor: newer.next_cursor,
        }))
    }
}

#[async_trait]
pub trait ChatRepository: Send + Sync {
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError>;
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError>;
    /// Chats with aggregate message/sync stats. Default: empty stats.
    async fn list_with_stats(&self, _sort: ChatSort) -> Result<Vec<ChatSummary>, RepositoryError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .map(|chat| ChatSummary {
                chat,
                stats: ChatStats::default(),
            })
            .collect())
    }
    async fn get_with_stats(&self, id: ChatId) -> Result<Option<ChatSummary>, RepositoryError> {
        Ok(self.get(id).await?.map(|chat| ChatSummary {
            chat,
            stats: ChatStats::default(),
        }))
    }
    /// Metadata-only upsert: must never change the tracking flag.
    async fn save_refresh(&self, chats: Vec<Chat>) -> Result<(), RepositoryError>;
    /// Idempotently sets the tracking flag; `None` when the chat is unknown.
    async fn set_tracked(&self, id: ChatId, tracked: bool)
    -> Result<Option<Chat>, RepositoryError>;
}

/// Read side of the opt-in collection scope, consulted by realtime ingestion for every batch so
/// tracking changes apply without a restart.
#[async_trait]
pub trait TrackingScope: Send + Sync {
    async fn tracked_chat_ids(&self) -> Result<std::collections::HashSet<ChatId>, RepositoryError>;
    /// Of `ids`, those already archived as non-channel (common-namespace) messages of a tracked chat.
    async fn archived_common_message_ids(
        &self,
        ids: &[MessageId],
    ) -> Result<std::collections::HashSet<MessageId>, RepositoryError>;
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
    /// State of the `--refetch` walk of a chat (none = no walk in progress).
    async fn get_refetch_checkpoint(
        &self,
        _chat_id: ChatId,
    ) -> Result<RefetchCheckpoint, RepositoryError> {
        Ok(RefetchCheckpoint::default())
    }
    /// Newest message ID archived for the chat; used to baseline catch-up without a gap.
    async fn newest_archived_id(
        &self,
        _chat_id: ChatId,
    ) -> Result<Option<MessageId>, RepositoryError> {
        Ok(None)
    }
    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError>;
    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError>;
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError>;
    /// Deletions whose chat could not be resolved (common-message tombstones).
    async fn unresolved_deletions(&self) -> Result<u64, RepositoryError> {
        Ok(0)
    }
    async fn list_chat_progress(
        &self,
        job_id: &str,
    ) -> Result<Vec<SyncChatProgress>, RepositoryError>;
    async fn recover_interrupted(&self) -> Result<(), RepositoryError>;
}

pub mod ingestion_worker;
pub mod pacer;
pub mod realtime;
pub mod services;
pub mod sync;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ChatKind, MessageId};
    use std::sync::Arc;

    fn filters() -> MessageFilters {
        MessageFilters {
            chat_id: None,
            sender_id: None,
            post_author: None,
            time_range: TimeRange::new(None, None).unwrap(),
            include_deleted: false,
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
            post_author: None,
            time_range: TimeRange {
                from: Some(DateTime::from_timestamp(2, 0).unwrap()),
                to: Some(DateTime::from_timestamp(1, 0).unwrap()),
            },
            include_deleted: false,
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
            sort: SearchSort::Relevance,
            offset: 0,
        };
        assert_eq!(too_long.validate(), Err(ValidationError::SearchTooLong));
        let punctuation = SearchMessagesQuery {
            text: " ！？\" * ".into(),
            ..too_long
        };
        assert_eq!(punctuation.validate(), Err(ValidationError::EmptySearch));
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
        async fn get(
            &self,
            _: ChatId,
            _: MessageId,
            _: bool,
        ) -> Result<Option<MessageView>, RepositoryError> {
            Ok(None)
        }
        async fn list(&self, _: ListMessagesQuery) -> Result<MessagePage, RepositoryError> {
            Ok(MessagePage {
                items: vec![],
                has_more: false,
                next_cursor: None,
                next_offset: None,
            })
        }
        async fn search(&self, _: SearchMessagesQuery) -> Result<MessagePage, RepositoryError> {
            Ok(MessagePage {
                items: vec![],
                has_more: false,
                next_cursor: None,
                next_offset: None,
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
        async fn set_tracked(&self, _: ChatId, _: bool) -> Result<Option<Chat>, RepositoryError> {
            Ok(None)
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

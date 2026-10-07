use super::ValidationError;
use crate::domain::{ChatId, Message, MessageId, SenderId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub(super) const MAX_SEARCH_LENGTH: usize = 4096;

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
    /// Hide messages whose sender is a known bot (`senders.is_bot = 1`). Unknown (NULL) senders
    /// and messages without a sender are kept.
    pub exclude_bots: bool,
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
    pub is_bot: Option<bool>,
    /// True when the sender is the bound Telegram account.
    pub is_self: bool,
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

use super::{PageSize, SenderInfo};
use crate::domain::{ChatId, ChatKind, SenderId, SenderKind};
use chrono::{DateTime, Utc};

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
    /// Only bots (`Some(true)`) or only non-bots (`Some(false)`, unknown included).
    pub is_bot: Option<bool>,
}

/// A (display name, username) combination observed for a sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderNameVersion {
    pub display_name: Option<String>,
    pub username: Option<String>,
}

/// One row of `sender_name_history`: when tgarchive observed the names (not when they changed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderNameHistoryEntry {
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// A sender with archive-wide aggregates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderProfile {
    pub id: SenderId,
    pub kind: SenderKind,
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub is_bot: Option<bool>,
    /// Set when a text query matched only a historical name; holds the matching old names.
    pub matched_history: Option<SenderNameVersion>,
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
    /// Observed names, newest observation first.
    pub name_history: Vec<SenderNameHistoryEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderPage {
    pub items: Vec<SenderProfile>,
    pub next_offset: Option<u64>,
}

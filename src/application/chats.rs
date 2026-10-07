use crate::domain::Chat;
use chrono::{DateTime, Utc};

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

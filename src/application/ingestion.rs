use super::ValidationError;
use crate::domain::{Chat, ChatId, MessageEvent, MessageId, Sender};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

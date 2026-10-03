use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use crate::{
    application::{MessagePage, SyncJob, SyncJobState, services::ApplicationStatus},
    domain::{Attachment, AttachmentKind, Chat, ChatKind, Message, SenderId},
};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ChatDto {
    pub id: i64,
    pub kind: ChatKindDto,
    pub title: Option<String>,
    pub username: Option<String>,
    /// Whether the chat is opted in for collection.
    pub tracked: bool,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatKindDto {
    Private,
    Group,
    Supergroup,
    Channel,
}

impl From<Chat> for ChatDto {
    fn from(chat: Chat) -> Self {
        Self {
            id: chat.id.get(),
            kind: match chat.kind {
                ChatKind::Private => ChatKindDto::Private,
                ChatKind::Group => ChatKindDto::Group,
                ChatKind::Supergroup => ChatKindDto::Supergroup,
                ChatKind::Channel => ChatKindDto::Channel,
            },
            title: chat.title,
            username: chat.username,
            tracked: chat.tracked,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessageDto {
    pub id: i64,
    pub chat_id: i64,
    pub sender_id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub text: Option<String>,
    pub reply_to: Option<i64>,
    pub attachments: Vec<AttachmentDto>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AttachmentDto {
    pub kind: AttachmentKindDto,
    pub telegram_file_id: Option<String>,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKindDto {
    Photo,
    Video,
    Audio,
    Voice,
    Document,
    Sticker,
    Animation,
    Other,
}

impl From<Attachment> for AttachmentDto {
    fn from(value: Attachment) -> Self {
        Self {
            kind: match value.kind {
                AttachmentKind::Photo => AttachmentKindDto::Photo,
                AttachmentKind::Video => AttachmentKindDto::Video,
                AttachmentKind::Audio => AttachmentKindDto::Audio,
                AttachmentKind::Voice => AttachmentKindDto::Voice,
                AttachmentKind::Document => AttachmentKindDto::Document,
                AttachmentKind::Sticker => AttachmentKindDto::Sticker,
                AttachmentKind::Animation => AttachmentKindDto::Animation,
                AttachmentKind::Other => AttachmentKindDto::Other,
            },
            telegram_file_id: value.telegram_file_id,
            mime_type: value.mime_type,
            file_name: value.file_name,
            size: value.size,
        }
    }
}

impl From<Message> for MessageDto {
    fn from(message: Message) -> Self {
        Self {
            id: message.id.get(),
            chat_id: message.chat_id.get(),
            sender_id: message.sender_id.map(SenderId::get),
            timestamp: message.timestamp,
            edited_at: message.edited_at,
            collected_at: message.collected_at,
            text: message.text,
            reply_to: message.reply_to.map(|id| id.get()),
            attachments: message.attachments.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessagePageDto {
    pub items: Vec<MessageDto>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

impl TryFrom<MessagePage> for MessagePageDto {
    type Error = crate::interface::cursor::CursorError;

    fn try_from(page: MessagePage) -> Result<Self, Self::Error> {
        Ok(Self {
            items: page.items.into_iter().map(Into::into).collect(),
            has_more: page.has_more,
            next_cursor: page
                .next_cursor
                .as_ref()
                .map(crate::interface::cursor::encode)
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct StatusDto {
    pub collector: ComponentStatusDto,
    pub sync_jobs: Vec<SyncJobDto>,
    /// Deletions that could not be attributed to a chat (kept as tombstones).
    pub unresolved_deletions: u64,
    /// Pacing of Telegram history requests; absent when this process has no Telegram access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimitDto>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RateLimitDto {
    /// Current minimum interval between history requests (grows after FLOOD_WAIT).
    pub interval_ms: u64,
    /// Configured base interval (`SYNC_PAGE_DELAY_MS`).
    pub base_interval_ms: u64,
    pub last_flood_wait_secs: Option<u64>,
    pub last_flood_at: Option<DateTime<Utc>>,
}

/// Response of `PUT /chats/{id}/tracking`: the chat, plus backfill info when requested.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TrackChatDto {
    #[serde(flatten)]
    pub chat: ChatDto,
    /// History sync job covering this chat (only with `backfill=true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfill_job_id: Option<String>,
    /// `queued` for a newly enqueued job, `already_running` when an existing job was reused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backfill: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ComponentStatusDto {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SyncJobDto {
    pub id: String,
    pub scope: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub has_error: bool,
}

impl From<ApplicationStatus> for StatusDto {
    fn from(status: ApplicationStatus) -> Self {
        Self {
            collector: ComponentStatusDto {
                state: status.collector.state.as_str().to_owned(),
                detail: status.collector.detail,
            },
            sync_jobs: status.sync_jobs.into_iter().map(Into::into).collect(),
            unresolved_deletions: status.unresolved_deletions,
            rate_limit: status.rate_limit.map(|r| RateLimitDto {
                interval_ms: r.interval_ms,
                base_interval_ms: r.base_interval_ms,
                last_flood_wait_secs: r.last_flood_wait_secs,
                last_flood_at: r.last_flood_at,
            }),
        }
    }
}

impl From<SyncJob> for SyncJobDto {
    fn from(job: SyncJob) -> Self {
        Self {
            id: job.id,
            scope: match job.scope {
                crate::application::SyncScope::All => "all".to_owned(),
                crate::application::SyncScope::Chat(id) => format!("chat:{}", id.get()),
            },
            state: job_state(job.state),
            created_at: job.created_at,
            started_at: job.started_at,
            completed_at: job.completed_at,
            has_error: job.summary_error.is_some(),
        }
    }
}

fn job_state(state: SyncJobState) -> String {
    match state {
        SyncJobState::Queued => "queued",
        SyncJobState::Running => "running",
        SyncJobState::Succeeded => "succeeded",
        SyncJobState::Failed => "failed",
        SyncJobState::Interrupted => "interrupted",
        SyncJobState::RateLimited => "rate_limited",
    }
    .to_owned()
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HealthDto {
    pub status: String,
}

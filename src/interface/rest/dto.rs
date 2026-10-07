use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    application::{
        ChatSort, ChatStats, ChatSummary, MessageContext, MessagePage, MessageView,
        SearchIndexStatus, SearchSort, SenderChatStat, SenderDetail, SenderInfo,
        SenderNameHistoryEntry, SenderNameVersion, SenderPage, SenderProfile, SenderSort,
        SenderSummary, SyncChatProgress, SyncJob, SyncJobState, services::ApplicationStatus,
    },
    domain::{Attachment, AttachmentKind, Chat, ChatKind, Forward, SenderId, SenderKind},
    infrastructure::persistence::sqlite::media::{MediaDownload, MediaPolicy},
};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MediaPolicyDto {
    pub auto_archive: bool,
}

impl From<MediaPolicy> for MediaPolicyDto {
    fn from(value: MediaPolicy) -> Self {
        Self {
            auto_archive: value.auto_archive,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MediaDownloadDto {
    pub id: i64,
    pub ordinal: i64,
    pub variant: String,
    pub state: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub byte_size: Option<i64>,
    pub next_attempt_at: Option<i64>,
    pub content_url: Option<String>,
    pub last_error: Option<String>,
}

impl From<MediaDownload> for MediaDownloadDto {
    fn from(value: MediaDownload) -> Self {
        let content_url =
            (value.state == "succeeded").then(|| format!("/api/v1/media/{}/content", value.id));
        Self {
            id: value.id,
            ordinal: value.ordinal,
            variant: value.variant,
            state: value.state,
            width: value.width,
            height: value.height,
            byte_size: value.byte_size,
            next_attempt_at: value.next_attempt_at,
            content_url,
            last_error: value.last_error,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ChatDto {
    pub id: i64,
    pub kind: ChatKindDto,
    pub title: Option<String>,
    pub username: Option<String>,
    /// Whether the chat is opted in for collection.
    pub tracked: bool,
    /// Aggregate archive/sync stats; omitted by `POST /chats/refresh`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<ChatStatsDto>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ChatStatsDto {
    /// Non-deleted archived messages.
    pub message_count: u64,
    pub deleted_count: u64,
    /// Timestamps of the oldest / newest non-deleted message.
    pub first_message_at: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    /// Whether history backfill reached the start of the chat.
    pub history_complete: bool,
    pub last_sync_completed_at: Option<DateTime<Utc>>,
    /// Sanitized reason of the last failed sync, cleared by the next successful checkpoint.
    pub last_error: Option<String>,
}

impl From<ChatStats> for ChatStatsDto {
    fn from(stats: ChatStats) -> Self {
        Self {
            message_count: stats.message_count,
            deleted_count: stats.deleted_count,
            first_message_at: stats.first_message_at,
            last_message_at: stats.last_message_at,
            history_complete: stats.history_complete,
            last_sync_completed_at: stats.last_sync_completed_at,
            last_error: stats.last_error,
        }
    }
}

/// Sort order of `GET /chats`.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatSortDto {
    /// Title, case-insensitive ascending (untitled last).
    Title,
    /// Newest last message first (chats without messages last).
    LastMessage,
    /// Most messages first.
    MessageCount,
}

impl From<ChatSortDto> for ChatSort {
    fn from(sort: ChatSortDto) -> Self {
        match sort {
            ChatSortDto::Title => Self::Title,
            ChatSortDto::LastMessage => Self::LastMessage,
            ChatSortDto::MessageCount => Self::MessageCount,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChatKindDto {
    Private,
    Group,
    Supergroup,
    Channel,
}

impl From<ChatSummary> for ChatDto {
    fn from(summary: ChatSummary) -> Self {
        let mut dto = Self::from(summary.chat);
        dto.stats = Some(summary.stats.into());
        dto
    }
}

impl From<Chat> for ChatDto {
    fn from(chat: Chat) -> Self {
        Self {
            stats: None,
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
    /// Resolved sender; null for messages without a sender.
    pub sender: Option<SenderDto>,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub text: Option<String>,
    pub reply_to: Option<i64>,
    pub attachments: Vec<AttachmentDto>,
    /// Channel post signature (`post_author`); not the sender.
    pub post_author: Option<String>,
    /// Origin of a forwarded message; null when the message was not forwarded.
    pub forward: Option<ForwardDto>,
    pub is_deleted: bool,
    /// Set when the message was deleted on Telegram (only returned with `include_deleted=true`).
    pub deleted_at: Option<DateTime<Utc>>,
    /// Search results only: excerpt (about 120 characters) around the first match, `null` when
    /// no match position could be determined. `text` always carries the full message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ForwardDto {
    /// Original author or channel (marked ID) when Telegram exposes it.
    pub from_id: Option<i64>,
    /// Name of the original author (also set for users who hide their account).
    pub from_name: Option<String>,
    /// When the original message was sent.
    pub date: Option<DateTime<Utc>>,
}

impl From<Forward> for ForwardDto {
    fn from(forward: Forward) -> Self {
        Self {
            from_id: forward.from_id.map(SenderId::get),
            from_name: forward.from_name,
            date: forward.date,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SenderKindDto {
    User,
    Chat,
    Channel,
    Unknown,
}

impl From<SenderKind> for SenderKindDto {
    fn from(kind: SenderKind) -> Self {
        match kind {
            SenderKind::User => Self::User,
            SenderKind::Chat => Self::Chat,
            SenderKind::Channel => Self::Channel,
            SenderKind::Unknown => Self::Unknown,
        }
    }
}

/// Sort order of `GET /senders`.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SenderSortDto {
    /// Most messages first (default).
    Messages,
    /// Most recently active first.
    LastMessage,
    /// Display name ascending (nameless last).
    Name,
}

impl From<SenderSortDto> for SenderSort {
    fn from(sort: SenderSortDto) -> Self {
        match sort {
            SenderSortDto::Messages => Self::Messages,
            SenderSortDto::LastMessage => Self::LastMessage,
            SenderSortDto::Name => Self::Name,
        }
    }
}

/// A sender with archive-wide aggregates.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderProfileDto {
    pub id: i64,
    pub kind: SenderKindDto,
    /// Profile name; for a channel posting as itself, the channel title.
    pub display_name: Option<String>,
    pub username: Option<String>,
    /// Telegram's bot flag; null = unknown or not a user.
    pub is_bot: Option<bool>,
    /// True when a text query matched only a historical name (see `matched_name`).
    pub matched_history: bool,
    /// The old display name/username that matched the query; null unless `matched_history`.
    pub matched_name: Option<SenderNameDto>,
    /// Messages by this sender (deleted ones only with `include_deleted=true`).
    pub message_count: u64,
    /// Chats in which the sender has messages.
    pub chat_count: u64,
    pub first_message_at: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    /// The Telegram account this archive is bound to.
    pub is_self: bool,
}

impl From<SenderProfile> for SenderProfileDto {
    fn from(profile: SenderProfile) -> Self {
        Self {
            id: profile.id.get(),
            kind: profile.kind.into(),
            display_name: profile.display_name,
            username: profile.username,
            is_bot: profile.is_bot,
            matched_history: profile.matched_history.is_some(),
            matched_name: profile.matched_history.map(Into::into),
            message_count: profile.message_count,
            chat_count: profile.chat_count,
            first_message_at: profile.first_message_at,
            last_message_at: profile.last_message_at,
            is_self: profile.is_self,
        }
    }
}

/// A historical display name / username combination.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderNameDto {
    pub display_name: Option<String>,
    pub username: Option<String>,
}

impl From<SenderNameVersion> for SenderNameDto {
    fn from(name: SenderNameVersion) -> Self {
        Self {
            display_name: name.display_name,
            username: name.username,
        }
    }
}

/// Names tgarchive observed for a sender. Timestamps are when tgarchive saw the names, not when
/// the user changed them.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderNameHistoryDto {
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

impl From<SenderNameHistoryEntry> for SenderNameHistoryDto {
    fn from(entry: SenderNameHistoryEntry) -> Self {
        Self {
            display_name: entry.display_name,
            username: entry.username,
            first_seen_at: entry.first_seen_at,
            last_seen_at: entry.last_seen_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderPageDto {
    pub items: Vec<SenderProfileDto>,
    /// Opaque cursor; pass it back as `cursor` to get the next page.
    pub next_cursor: Option<String>,
}

impl TryFrom<SenderPage> for SenderPageDto {
    type Error = crate::interface::cursor::CursorError;

    fn try_from(page: SenderPage) -> Result<Self, Self::Error> {
        Ok(Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next_cursor: page
                .next_offset
                .map(crate::interface::cursor::encode_offset)
                .transpose()?,
        })
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderChatDto {
    pub chat_id: i64,
    pub title: Option<String>,
    pub kind: ChatKindDto,
    pub message_count: u64,
    pub last_message_at: Option<DateTime<Utc>>,
}

impl From<SenderChatStat> for SenderChatDto {
    fn from(stat: SenderChatStat) -> Self {
        Self {
            chat_id: stat.chat_id.get(),
            title: stat.title,
            kind: match stat.kind {
                ChatKind::Private => ChatKindDto::Private,
                ChatKind::Group => ChatKindDto::Group,
                ChatKind::Supergroup => ChatKindDto::Supergroup,
                ChatKind::Channel => ChatKindDto::Channel,
            },
            message_count: stat.message_count,
            last_message_at: stat.last_message_at,
        }
    }
}

/// A sender plus its per-chat breakdown (most messages first).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderDetailDto {
    pub id: i64,
    pub kind: SenderKindDto,
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub is_bot: Option<bool>,
    pub message_count: u64,
    pub chat_count: u64,
    pub first_message_at: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub is_self: bool,
    pub chats: Vec<SenderChatDto>,
    /// Observed names, newest observation first.
    pub name_history: Vec<SenderNameHistoryDto>,
}

impl From<SenderDetail> for SenderDetailDto {
    fn from(detail: SenderDetail) -> Self {
        let profile = SenderProfileDto::from(detail.profile);
        Self {
            id: profile.id,
            kind: profile.kind,
            display_name: profile.display_name,
            username: profile.username,
            is_bot: profile.is_bot,
            message_count: profile.message_count,
            chat_count: profile.chat_count,
            first_message_at: profile.first_message_at,
            last_message_at: profile.last_message_at,
            is_self: profile.is_self,
            chats: detail.chats.into_iter().map(Into::into).collect(),
            name_history: detail.name_history.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderDto {
    pub id: i64,
    /// Profile name; for a chat/channel posting as itself, the chat title.
    pub display_name: Option<String>,
    pub username: Option<String>,
    /// Telegram's bot flag; null = unknown or not a user.
    pub is_bot: Option<bool>,
    /// True when the sender is the bound Telegram account.
    pub is_self: bool,
}

impl From<SenderInfo> for SenderDto {
    fn from(sender: SenderInfo) -> Self {
        Self {
            id: sender.id.get(),
            display_name: sender.display_name,
            username: sender.username,
            is_bot: sender.is_bot,
            is_self: sender.is_self,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SenderSummaryDto {
    pub id: i64,
    pub display_name: Option<String>,
    pub username: Option<String>,
    pub is_bot: Option<bool>,
    /// Non-deleted messages by this sender in the chat.
    pub message_count: u64,
}

impl From<SenderSummary> for SenderSummaryDto {
    fn from(summary: SenderSummary) -> Self {
        Self {
            id: summary.sender.id.get(),
            display_name: summary.sender.display_name,
            username: summary.sender.username,
            is_bot: summary.sender.is_bot,
            message_count: summary.message_count,
        }
    }
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

impl From<MessageView> for MessageDto {
    fn from(view: MessageView) -> Self {
        let MessageView {
            message,
            sender,
            deleted_at,
            snippet,
        } = view;
        Self {
            snippet,
            sender: sender.map(Into::into),
            is_deleted: deleted_at.is_some(),
            deleted_at,
            id: message.id.get(),
            chat_id: message.chat_id.get(),
            sender_id: message.sender_id.map(SenderId::get),
            timestamp: message.timestamp,
            edited_at: message.edited_at,
            collected_at: message.collected_at,
            text: message.text,
            reply_to: message.reply_to.map(|id| id.get()),
            attachments: message.attachments.into_iter().map(Into::into).collect(),
            post_author: message.post_author,
            forward: message.forward.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessagePageDto {
    pub items: Vec<MessageDto>,
    pub has_more: bool,
    /// Opaque cursor; pass it back as `before` to get the next page.
    pub next_cursor: Option<String>,
}

/// Result order of `GET /messages/search`.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchSortDto {
    /// BM25 relevance, best first (default). The cursor is a position, so pages are stable
    /// unless messages are written between requests.
    Relevance,
    /// Newest first with keyset cursors. Also what relevance falls back to for single-character
    /// queries and while the search index is not ready.
    Time,
}

impl From<SearchSortDto> for SearchSort {
    fn from(sort: SearchSortDto) -> Self {
        match sort {
            SearchSortDto::Relevance => Self::Relevance,
            SearchSortDto::Time => Self::Time,
        }
    }
}

impl TryFrom<MessagePage> for MessagePageDto {
    type Error = crate::interface::cursor::CursorError;

    fn try_from(page: MessagePage) -> Result<Self, Self::Error> {
        Ok(Self {
            items: page.items.into_iter().map(Into::into).collect(),
            has_more: page.has_more,
            next_cursor: match (page.next_cursor.as_ref(), page.next_offset) {
                (Some(cursor), _) => Some(crate::interface::cursor::encode(cursor)?),
                (None, Some(offset)) => Some(crate::interface::cursor::encode_offset(offset)?),
                (None, None) => None,
            },
        })
    }
}

/// Messages around an anchor, both lists oldest first.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MessageContextDto {
    pub anchor: MessageDto,
    pub before: Vec<MessageDto>,
    pub after: Vec<MessageDto>,
    pub has_more_before: bool,
    pub has_more_after: bool,
    /// Pass as `before` to `GET .../messages` to keep scrolling to older messages.
    pub before_cursor: Option<String>,
    /// Pass as `after` to `GET .../messages` to keep scrolling to newer messages.
    pub after_cursor: Option<String>,
}

impl TryFrom<MessageContext> for MessageContextDto {
    type Error = crate::interface::cursor::CursorError;

    fn try_from(context: MessageContext) -> Result<Self, Self::Error> {
        let encode = |cursor: Option<crate::application::MessageCursor>| {
            cursor
                .as_ref()
                .map(crate::interface::cursor::encode)
                .transpose()
        };
        Ok(Self {
            anchor: context.anchor.into(),
            before: context.before.into_iter().map(Into::into).collect(),
            after: context.after.into_iter().map(Into::into).collect(),
            has_more_before: context.has_more_before,
            has_more_after: context.has_more_after,
            before_cursor: encode(context.before_cursor)?,
            after_cursor: encode(context.after_cursor)?,
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
    /// Full-text search index state.
    pub search_index: SearchIndexDto,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SearchIndexDto {
    /// Index format version stored in the database (0 = never built).
    pub version: u32,
    /// `ready`; `rebuilding` or `stale` (search then uses a slower substring scan until
    /// `tgarchive search rebuild-index` / `db init` completes).
    pub state: String,
    /// Messages in the index.
    pub indexed: u64,
    /// Messages with text that belong in the index.
    pub total: u64,
}

impl From<SearchIndexStatus> for SearchIndexDto {
    fn from(status: SearchIndexStatus) -> Self {
        Self {
            version: status.version,
            state: status.state.as_str().to_owned(),
            indexed: status.indexed,
            total: status.total,
        }
    }
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
    /// Sanitized single-line failure reason (at most 300 chars; never message text).
    pub error_summary: Option<String>,
    /// Seconds Telegram asked to wait, for `rate_limited` jobs.
    pub retry_after_secs: Option<u64>,
    /// Per-chat progress; only present on `GET /sync/jobs/{id}`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chats: Option<Vec<SyncJobChatDto>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SyncJobChatDto {
    pub chat_id: i64,
    pub title: Option<String>,
    pub state: String,
    pub committed_count: u64,
    pub error_summary: Option<String>,
}

const MAX_SUMMARY_CHARS: usize = 300;

fn bound_summary(text: String) -> String {
    let single_line = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let single_line = single_line.split_whitespace().collect::<Vec<_>>().join(" ");
    single_line.chars().take(MAX_SUMMARY_CHARS).collect()
}

/// Parses the `retry after N s` produced for FLOOD_WAIT failures.
fn parse_retry_after(summary: &str) -> Option<u64> {
    let rest = summary.split("retry after ").nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

impl SyncJobDto {
    pub fn with_chats(
        mut self,
        progress: Vec<SyncChatProgress>,
        titles: &HashMap<i64, String>,
    ) -> Self {
        self.chats = Some(
            progress
                .into_iter()
                .map(|p| SyncJobChatDto {
                    chat_id: p.chat_id.get(),
                    title: titles.get(&p.chat_id.get()).cloned(),
                    state: job_state(p.state),
                    committed_count: p.committed_messages,
                    error_summary: p.summary_error.map(bound_summary),
                })
                .collect(),
        );
        self
    }
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
            search_index: status.search_index.into(),
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
        let error_summary = job.summary_error.map(bound_summary);
        let retry_after_secs = if job.state == SyncJobState::RateLimited {
            error_summary.as_deref().and_then(parse_retry_after)
        } else {
            None
        };
        Self {
            has_error: error_summary.is_some(),
            error_summary,
            retry_after_secs,
            chats: None,
            id: job.id,
            scope: match job.scope {
                crate::application::SyncScope::All => "all".to_owned(),
                crate::application::SyncScope::Chat(id) => format!("chat:{}", id.get()),
            },
            state: job_state(job.state),
            created_at: job.created_at,
            started_at: job.started_at,
            completed_at: job.completed_at,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_are_single_line_bounded_and_retry_after_is_parsed() {
        assert_eq!(bound_summary("a\nb\tc".into()), "a b c");
        assert_eq!(bound_summary("x".repeat(500)).chars().count(), 300);
        assert_eq!(
            parse_retry_after("Telegram rate limit: retry after 42 s"),
            Some(42)
        );
        assert_eq!(parse_retry_after("boom"), None);
    }
}

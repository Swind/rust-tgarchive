use super::*;

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

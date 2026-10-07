use super::*;

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

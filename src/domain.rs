use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CHANNEL_MARK: i64 = 1_000_000_000_000;
const MAX_USER_ID: i64 = 0xffffffffff;
const MAX_BASIC_GROUP_ID: i64 = 999_999_999_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct ChatId(i64);

impl ChatId {
    pub fn from_telegram(kind: ChatKind, raw_id: i64) -> Result<Self, IdError> {
        if raw_id <= 0 {
            return Err(IdError::InvalidRawId);
        }
        let marked = match kind {
            ChatKind::Private if raw_id <= MAX_USER_ID => raw_id,
            ChatKind::Private => return Err(IdError::OutOfRange),
            ChatKind::Group if raw_id <= MAX_BASIC_GROUP_ID => -raw_id,
            ChatKind::Group => return Err(IdError::OutOfRange),
            ChatKind::Supergroup | ChatKind::Channel => CHANNEL_MARK
                .checked_add(raw_id)
                .and_then(i64::checked_neg)
                .ok_or(IdError::OutOfRange)?,
        };
        Ok(Self(marked))
    }

    pub fn from_marked(value: i64) -> Result<Self, IdError> {
        if value == 0 || value == -CHANNEL_MARK {
            return Err(IdError::InvalidMarkedId);
        }
        if value > MAX_USER_ID
            || (value < 0 && value > -CHANNEL_MARK && -value > MAX_BASIC_GROUP_ID)
        {
            return Err(IdError::OutOfRange);
        }
        if value < -CHANNEL_MARK
            && value
                .checked_neg()
                .and_then(|absolute| absolute.checked_sub(CHANNEL_MARK))
                .is_none()
        {
            return Err(IdError::OutOfRange);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for ChatId {
    type Error = IdError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::from_marked(value)
    }
}

impl From<ChatId> for i64 {
    fn from(value: ChatId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct SenderId(i64);

impl SenderId {
    pub fn from_telegram(kind: SenderKind, raw_id: i64) -> Result<Self, IdError> {
        let chat_kind = match kind {
            SenderKind::User => ChatKind::Private,
            SenderKind::Chat => ChatKind::Group,
            SenderKind::Channel => ChatKind::Channel,
            SenderKind::Unknown => return Err(IdError::UnknownKind),
        };
        ChatId::from_telegram(chat_kind, raw_id).map(|id| Self(id.0))
    }

    pub fn from_marked(value: i64) -> Result<Self, IdError> {
        ChatId::from_marked(value).map(|id| Self(id.0))
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for SenderId {
    type Error = IdError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        ChatId::from_marked(value).map(|id| Self(id.0))
    }
}

impl From<SenderId> for i64 {
    fn from(value: SenderId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct MessageId(i64);

impl MessageId {
    /// Catch-up baseline for a chat with no messages yet; never a real Telegram message ID.
    pub const BEFORE_FIRST: Self = Self(0);

    pub fn new(value: i64) -> Result<Self, IdError> {
        if value <= 0 || value > i64::from(i32::MAX) {
            return Err(IdError::InvalidRawId);
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for MessageId {
    type Error = IdError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MessageId> for i64 {
    fn from(value: MessageId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatKind {
    Private,
    Group,
    Supergroup,
    Channel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SenderKind {
    User,
    Chat,
    Channel,
    Unknown,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    #[error("Telegram identifier must be positive")]
    InvalidRawId,
    #[error("Telegram identifier is outside the supported signed ID range")]
    OutOfRange,
    #[error("marked ID is zero or falls in the reserved namespace gap")]
    InvalidMarkedId,
    #[error("an unknown sender has no Telegram identifier namespace")]
    UnknownKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chat {
    pub id: ChatId,
    pub kind: ChatKind,
    pub title: Option<String>,
    pub username: Option<String>,
    /// Whether the chat is explicitly opted in for collection. Telegram metadata never sets it.
    #[serde(default)]
    pub tracked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sender {
    pub id: SenderId,
    pub kind: SenderKind,
    pub display_name: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub chat_id: ChatId,
    pub sender_id: Option<SenderId>,
    pub timestamp: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub collected_at: DateTime<Utc>,
    pub text: Option<String>,
    pub reply_to: Option<MessageId>,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub kind: AttachmentKind,
    pub telegram_file_id: Option<String>,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
    Photo,
    Video,
    Audio,
    Voice,
    Document,
    Sticker,
    Animation,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageEvent {
    Created(Message),
    Updated(Message),
    Deleted {
        chat_id: ChatId,
        message_id: MessageId,
        deleted_at: DateTime<Utc>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ids_are_disjoint_and_checked_at_boundaries() {
        let user = ChatId::from_telegram(ChatKind::Private, 42).unwrap();
        let group = ChatId::from_telegram(ChatKind::Group, 42).unwrap();
        let channel = ChatId::from_telegram(ChatKind::Channel, 42).unwrap();
        assert_eq!(
            (user.get(), group.get(), channel.get()),
            (42, -42, -1_000_000_000_042)
        );
        for (kind, raw) in [
            (ChatKind::Private, 42),
            (ChatKind::Group, 42),
            (ChatKind::Channel, 42),
        ] {
            let marked = ChatId::from_telegram(kind, raw).unwrap();
            assert_eq!(ChatId::from_marked(marked.get()), Ok(marked));
        }
        assert_ne!(user, group);
        assert_ne!(group, channel);
        assert!(ChatId::from_telegram(ChatKind::Group, CHANNEL_MARK).is_err());
        assert!(ChatId::from_telegram(ChatKind::Private, MAX_USER_ID + 1).is_err());
        assert!(ChatId::from_telegram(ChatKind::Group, MAX_BASIC_GROUP_ID + 1).is_err());
        assert!(ChatId::from_telegram(ChatKind::Channel, i64::MAX).is_err());
        assert!(ChatId::from_marked(-CHANNEL_MARK).is_err());
        assert!(ChatId::from_marked(i64::MIN).is_err());
        assert!(serde_json::from_str::<ChatId>("0").is_err());
        assert!(serde_json::from_str::<MessageId>("0").is_err());
        assert!(MessageId::new(i64::from(i32::MAX)).is_ok());
        assert!(MessageId::new(i64::from(i32::MAX) + 1).is_err());
        assert!(serde_json::from_str::<MessageId>("2147483648").is_err());
    }

    #[test]
    fn absent_sender_stays_absent_and_attachments_allow_missing_metadata() {
        let attachment = Attachment {
            kind: AttachmentKind::Photo,
            telegram_file_id: None,
            mime_type: None,
            file_name: None,
            size: None,
        };
        assert_eq!(attachment.size, None);
        assert!(SenderId::from_telegram(SenderKind::Unknown, 5).is_err());
    }
}

use chrono::{DateTime, Utc};
use grammers_client::{media::Media, message::Message as TelegramMessage, peer::Peer};
use grammers_session::types::{PeerId, PeerKind};

use crate::domain::{
    Attachment, AttachmentKind, Chat, ChatId, ChatKind, Forward, IdError, Message, MessageId,
    Sender, SenderId, SenderKind,
};

#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    #[error("Telegram peer identifier is not representable by the archive ID model")]
    InvalidPeerId,
    #[error("Telegram message timestamp is outside the supported UTC range")]
    InvalidTimestamp,
    #[error("Telegram update contains an unsupported message constructor")]
    UnsupportedMessage,
    #[error(transparent)]
    InvalidId(#[from] IdError),
}

pub fn chat_id(peer_id: PeerId) -> Result<ChatId, MappingError> {
    let value = peer_id
        .bot_api_dialog_id()
        .ok_or(MappingError::InvalidPeerId)?;
    ChatId::from_marked(value).map_err(Into::into)
}

pub fn sender_id(peer_id: PeerId) -> Result<SenderId, MappingError> {
    let value = peer_id
        .bot_api_dialog_id()
        .ok_or(MappingError::InvalidPeerId)?;
    SenderId::from_marked(value).map_err(Into::into)
}

pub fn map_chat(peer: &Peer) -> Result<Chat, MappingError> {
    let (kind, title, username) = match peer {
        Peer::User(user) => (
            ChatKind::Private,
            display_name(user.first_name(), user.last_name()),
            user.username().map(str::to_owned),
        ),
        Peer::Group(group) if group.id().kind() == PeerKind::Chat => (
            ChatKind::Group,
            group.title().map(str::to_owned),
            group.username().map(str::to_owned),
        ),
        Peer::Group(group) => (
            ChatKind::Supergroup,
            group.title().map(str::to_owned),
            group.username().map(str::to_owned),
        ),
        Peer::Channel(channel) => (
            ChatKind::Channel,
            Some(channel.title().to_owned()),
            channel.username().map(str::to_owned),
        ),
    };

    Ok(Chat {
        id: chat_id(peer.id())?,
        kind,
        title,
        username,
        tracked: false,
    })
}

pub fn map_sender(peer_id: PeerId, peer: Option<&Peer>) -> Result<Sender, MappingError> {
    let kind = match peer_id.kind() {
        PeerKind::User => SenderKind::User,
        PeerKind::Chat => SenderKind::Chat,
        PeerKind::Channel => SenderKind::Channel,
    };
    let (display_name, username, is_bot) = match peer {
        Some(Peer::User(user)) => (
            display_name(user.first_name(), user.last_name()),
            user.username().map(str::to_owned),
            user_is_bot(&user.raw),
        ),
        Some(Peer::Group(group)) => (
            group.title().map(str::to_owned),
            group.username().map(str::to_owned),
            None,
        ),
        Some(Peer::Channel(channel)) => (
            Some(channel.title().to_owned()),
            channel.username().map(str::to_owned),
            None,
        ),
        None => (None, None, None),
    };
    Ok(Sender {
        id: sender_id(peer_id)?,
        kind,
        display_name,
        username,
        is_bot,
    })
}

/// `account` is the Telegram account the archive is bound to; it attributes outgoing messages
/// that carry no `from_id`.
pub fn map_message(
    message: &TelegramMessage,
    collected_at: DateTime<Utc>,
    account: Option<SenderId>,
) -> Result<Message, MappingError> {
    map_raw_message(&message.raw, collected_at, account)
}

/// Profile of the authenticated account as a sender row.
pub fn map_account(user: &grammers_client::peer::User) -> Result<Sender, MappingError> {
    Ok(Sender {
        id: sender_id(user.id())?,
        kind: SenderKind::User,
        display_name: display_name(user.first_name(), user.last_name()),
        username: user.username().map(str::to_owned),
        is_bot: user_is_bot(&user.raw),
    })
}

/// Telegram's `bot` flag; `None` for empty/deleted user stubs, which carry no profile data.
/// Only users get a value: chats and channels stay `None` (not applicable).
fn user_is_bot(raw: &grammers_client::tl::enums::User) -> Option<bool> {
    match raw {
        grammers_client::tl::enums::User::User(user) => Some(user.bot),
        grammers_client::tl::enums::User::Empty(_) => None,
    }
}

/// Maps message and service-message TL fixtures without requiring a live peer cache.
pub fn map_raw_message(
    raw: &grammers_client::tl::enums::Message,
    collected_at: DateTime<Utc>,
    account: Option<SenderId>,
) -> Result<Message, MappingError> {
    use grammers_client::tl::enums::Message as RawMessage;

    let (
        id,
        raw_peer,
        raw_sender_id,
        outgoing,
        post,
        date,
        edited_at,
        text,
        reply_to,
        raw_media,
        post_author,
        fwd_from,
    ) = match raw {
        RawMessage::Message(message) => (
            message.id,
            &message.peer_id,
            message.from_id.as_ref(),
            message.out,
            message.post,
            message.date,
            message.edit_date,
            Some(message.message.clone()),
            message.reply_to.as_ref().and_then(reply_message_id),
            message.media.clone(),
            message
                .post_author
                .clone()
                .filter(|author| !author.is_empty()),
            message.fwd_from.as_ref(),
        ),
        RawMessage::Service(message) => (
            message.id,
            &message.peer_id,
            message.from_id.as_ref(),
            message.out,
            message.post,
            message.date,
            None,
            None,
            message.reply_to.as_ref().and_then(reply_message_id),
            None,
            None,
            None,
        ),
        RawMessage::Empty(_) => return Err(MappingError::UnsupportedMessage),
    };
    let timestamp =
        DateTime::from_timestamp(i64::from(date), 0).ok_or(MappingError::InvalidTimestamp)?;
    let peer_id = raw_peer_id(raw_peer)?;
    // Telegram omits `from_id` for outgoing messages, incoming private messages and channel
    // posts. Outgoing ones belong to the bound account, private ones to the peer, posts to
    // the channel itself.
    let resolved_sender = match raw_sender_id {
        Some(raw) => Some(sender_id(raw_peer_id(raw)?)?),
        None if outgoing && !post && account.is_some() => account,
        None if matches!(peer_id.kind(), PeerKind::Channel)
            || (matches!(peer_id.kind(), PeerKind::User) && !outgoing) =>
        {
            Some(sender_id(peer_id)?)
        }
        None => None,
    };
    let forward = fwd_from
        .map(|header| {
            let grammers_client::tl::enums::MessageFwdHeader::Header(header) = header;
            Ok::<_, MappingError>(Forward {
                from_id: header
                    .from_id
                    .as_ref()
                    .map(|peer| sender_id(raw_peer_id(peer)?))
                    .transpose()?,
                from_name: header.from_name.clone().filter(|name| !name.is_empty()),
                date: DateTime::from_timestamp(i64::from(header.date), 0),
            })
        })
        .transpose()?;
    let attachments = raw_media
        .and_then(Media::from_raw)
        .into_iter()
        .map(map_attachment)
        .collect();
    Ok(Message {
        id: MessageId::new(i64::from(id))?,
        chat_id: chat_id(peer_id)?,
        sender_id: resolved_sender,
        timestamp,
        edited_at: edited_at
            .map(|value| {
                DateTime::from_timestamp(i64::from(value), 0).ok_or(MappingError::InvalidTimestamp)
            })
            .transpose()?,
        collected_at,
        text,
        reply_to: reply_to
            .map(|id| MessageId::new(i64::from(id)))
            .transpose()?,
        attachments,
        post_author,
        forward,
    })
}

fn reply_message_id(reply: &grammers_client::tl::enums::MessageReplyHeader) -> Option<i32> {
    match reply {
        grammers_client::tl::enums::MessageReplyHeader::Header(header) => header.reply_to_msg_id,
        grammers_client::tl::enums::MessageReplyHeader::MessageReplyStoryHeader(_) => None,
    }
}

fn raw_peer_id(peer: &grammers_client::tl::enums::Peer) -> Result<PeerId, MappingError> {
    match peer {
        grammers_client::tl::enums::Peer::User(peer) => {
            PeerId::user(peer.user_id).ok_or(MappingError::InvalidPeerId)
        }
        grammers_client::tl::enums::Peer::Chat(peer) => {
            PeerId::chat(peer.chat_id).ok_or(MappingError::InvalidPeerId)
        }
        grammers_client::tl::enums::Peer::Channel(peer) => {
            PeerId::channel(peer.channel_id).ok_or(MappingError::InvalidPeerId)
        }
    }
}

fn map_attachment(media: Media) -> Attachment {
    let (kind, telegram_file_id, mime_type, file_name, size) = match media {
        Media::Photo(photo) => (
            AttachmentKind::Photo,
            Some(photo.id().to_string()),
            None,
            None,
            photo.size().and_then(|size| i64::try_from(size).ok()),
        ),
        Media::Sticker(sticker) => (
            AttachmentKind::Sticker,
            Some(sticker.document.id().to_string()),
            sticker.document.mime_type().map(str::to_owned),
            sticker.document.name().map(str::to_owned),
            sticker
                .document
                .size()
                .and_then(|size| i64::try_from(size).ok()),
        ),
        Media::Document(document) => {
            let mime = document.mime_type().map(str::to_owned);
            let kind = match mime.as_deref() {
                Some(mime) if mime.starts_with("video/") => AttachmentKind::Video,
                Some(mime) if mime.starts_with("audio/") => AttachmentKind::Audio,
                _ => AttachmentKind::Document,
            };
            (
                kind,
                Some(document.id().to_string()),
                mime,
                document.name().map(str::to_owned),
                document.size().and_then(|size| i64::try_from(size).ok()),
            )
        }
        _ => (AttachmentKind::Other, None, None, None, None),
    };

    Attachment {
        kind,
        telegram_file_id,
        mime_type,
        file_name,
        size,
    }
}

fn display_name(first: Option<&str>, last: Option<&str>) -> Option<String> {
    let name = [first, last]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
#[path = "mapper/tests.rs"]
mod tests;

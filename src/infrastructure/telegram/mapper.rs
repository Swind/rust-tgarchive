use chrono::{DateTime, Utc};
use grammers_client::{media::Media, message::Message as TelegramMessage, peer::Peer};
use grammers_session::types::{PeerId, PeerKind};

use crate::domain::{
    Attachment, AttachmentKind, Chat, ChatId, ChatKind, IdError, Message, MessageId, Sender,
    SenderId, SenderKind,
};

#[derive(Debug, thiserror::Error)]
pub enum MappingError {
    #[error("Telegram peer identifier is not representable by the archive ID model")]
    InvalidPeerId,
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
    })
}

pub fn map_sender(peer_id: PeerId, peer: Option<&Peer>) -> Result<Sender, MappingError> {
    let kind = match peer_id.kind() {
        PeerKind::User => SenderKind::User,
        PeerKind::Chat => SenderKind::Chat,
        PeerKind::Channel => SenderKind::Channel,
    };
    let (display_name, username) = match peer {
        Some(Peer::User(user)) => (
            display_name(user.first_name(), user.last_name()),
            user.username().map(str::to_owned),
        ),
        Some(Peer::Group(group)) => (
            group.title().map(str::to_owned),
            group.username().map(str::to_owned),
        ),
        Some(Peer::Channel(channel)) => (
            Some(channel.title().to_owned()),
            channel.username().map(str::to_owned),
        ),
        None => (None, None),
    };
    Ok(Sender {
        id: sender_id(peer_id)?,
        kind,
        display_name,
        username,
    })
}

pub fn map_message(
    message: &TelegramMessage,
    collected_at: DateTime<Utc>,
) -> Result<Message, MappingError> {
    Ok(Message {
        id: MessageId::new(i64::from(message.id()))?,
        chat_id: chat_id(message.peer_id())?,
        sender_id: message.sender_id().map(sender_id).transpose()?,
        timestamp: message.date(),
        edited_at: message.edit_date(),
        collected_at,
        text: Some(message.text().to_owned()),
        reply_to: message
            .reply_to_message_id()
            .map(|id| MessageId::new(i64::from(id)))
            .transpose()?,
        attachments: message.media().into_iter().map(map_attachment).collect(),
    })
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
mod tests {
    use super::*;

    #[test]
    fn grammers_peer_ids_map_to_domain_marked_ids() {
        assert_eq!(chat_id(PeerId::user(42).unwrap()).unwrap().get(), 42);
        assert_eq!(chat_id(PeerId::chat(42).unwrap()).unwrap().get(), -42);
        assert_eq!(
            chat_id(PeerId::channel(42).unwrap()).unwrap().get(),
            -1_000_000_000_042
        );
        assert_eq!(sender_id(PeerId::user(42).unwrap()).unwrap().get(), 42);
        assert_eq!(sender_id(PeerId::chat(42).unwrap()).unwrap().get(), -42);
        assert_eq!(
            sender_id(PeerId::channel(42).unwrap()).unwrap().get(),
            -1_000_000_000_042
        );
    }

    #[test]
    fn self_user_without_known_numeric_id_is_not_fabricated() {
        assert!(matches!(
            chat_id(PeerId::self_user()),
            Err(MappingError::InvalidPeerId)
        ));
        assert!(matches!(
            sender_id(PeerId::self_user()),
            Err(MappingError::InvalidPeerId)
        ));
    }

    #[test]
    fn display_names_trim_absent_parts_without_empty_names() {
        assert_eq!(
            display_name(Some("Ada"), Some("Lovelace")),
            Some("Ada Lovelace".into())
        );
        assert_eq!(display_name(Some("Ada"), None), Some("Ada".into()));
        assert_eq!(display_name(None, Some("")), None);
    }
}

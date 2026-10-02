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
    map_raw_message(&message.raw, collected_at)
}

/// Maps message and service-message TL fixtures without requiring a live peer cache.
pub fn map_raw_message(
    raw: &grammers_client::tl::enums::Message,
    collected_at: DateTime<Utc>,
) -> Result<Message, MappingError> {
    use grammers_client::tl::enums::Message as RawMessage;

    let (id, raw_peer, raw_sender_id, date, edited_at, text, reply_to, raw_media) = match raw {
        RawMessage::Message(message) => (
            message.id,
            &message.peer_id,
            message.from_id.as_ref(),
            message.date,
            message.edit_date,
            Some(message.message.clone()),
            message.reply_to.as_ref().and_then(reply_message_id),
            message.media.clone(),
        ),
        RawMessage::Service(message) => (
            message.id,
            &message.peer_id,
            message.from_id.as_ref(),
            message.date,
            None,
            None,
            message.reply_to.as_ref().and_then(reply_message_id),
            None,
        ),
        RawMessage::Empty(_) => return Err(MappingError::UnsupportedMessage),
    };
    let timestamp =
        DateTime::from_timestamp(i64::from(date), 0).ok_or(MappingError::InvalidTimestamp)?;
    let peer_id = raw_peer_id(raw_peer)?;
    let sender_id = raw_sender_id
        .map(raw_peer_id)
        .transpose()?
        .map(sender_id)
        .transpose()?;
    let attachments = raw_media
        .and_then(Media::from_raw)
        .into_iter()
        .map(map_attachment)
        .collect();
    Ok(Message {
        id: MessageId::new(i64::from(id))?,
        chat_id: chat_id(peer_id)?,
        sender_id,
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
mod tests {
    use super::*;

    fn raw_message(
        media: Option<grammers_client::tl::enums::MessageMedia>,
    ) -> grammers_client::tl::enums::Message {
        use grammers_client::tl::{enums::Peer, types};

        types::Message {
            out: false,
            mentioned: false,
            media_unread: false,
            silent: false,
            post: false,
            from_scheduled: false,
            legacy: false,
            edit_hide: false,
            pinned: false,
            noforwards: false,
            invert_media: false,
            offline: false,
            video_processing_pending: false,
            paid_suggested_post_stars: false,
            paid_suggested_post_ton: false,
            id: 123,
            from_id: Some(Peer::User(types::PeerUser { user_id: 99 })),
            from_boosts_applied: None,
            from_rank: None,
            peer_id: Peer::User(types::PeerUser { user_id: 42 }),
            saved_peer_id: None,
            fwd_from: None,
            via_bot_id: None,
            via_business_bot_id: None,
            guestchat_via_from: None,
            reply_to: Some(grammers_client::tl::enums::MessageReplyHeader::Header(
                types::MessageReplyHeader {
                    reply_to_scheduled: false,
                    forum_topic: false,
                    quote: false,
                    reply_to_ephemeral: false,
                    reply_to_msg_id: Some(7),
                    reply_to_peer_id: None,
                    reply_from: None,
                    reply_media: None,
                    reply_to_top_id: None,
                    quote_text: None,
                    quote_entities: None,
                    quote_offset: None,
                    todo_item_id: None,
                    poll_option: None,
                },
            )),
            date: 1_700_000_000,
            message: "fixture text".into(),
            media,
            reply_markup: None,
            entities: None,
            views: None,
            forwards: None,
            replies: None,
            edit_date: None,
            post_author: None,
            grouped_id: None,
            reactions: None,
            restriction_reason: None,
            ttl_period: None,
            quick_reply_shortcut_id: None,
            effect: None,
            factcheck: None,
            report_delivery_until_date: None,
            paid_message_stars: None,
            suggested_post: None,
            schedule_repeat_period: None,
            summary_from_language: None,
            rich_message: None,
        }
        .into()
    }

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

    #[test]
    fn raw_message_media_and_reply_fixture_maps_to_domain_message() {
        use grammers_client::tl::{enums, types};

        let media = enums::MessageMedia::Document(types::MessageMediaDocument {
            nopremium: false,
            spoiler: false,
            video: true,
            round: false,
            voice: false,
            document: Some(enums::Document::Document(types::Document {
                id: 456,
                access_hash: 1,
                file_reference: Vec::new(),
                date: 1_700_000_000,
                mime_type: "video/mp4".into(),
                size: 2048,
                thumbs: None,
                video_thumbs: None,
                dc_id: 2,
                attributes: Vec::new(),
            })),
            alt_documents: None,
            video_cover: None,
            video_timestamp: None,
            ttl_seconds: None,
        });
        let mapped = map_raw_message(&raw_message(Some(media)), Utc::now()).unwrap();

        assert_eq!(mapped.id.get(), 123);
        assert_eq!(mapped.chat_id.get(), 42);
        assert_eq!(mapped.sender_id.unwrap().get(), 99);
        assert_eq!(mapped.reply_to.unwrap().get(), 7);
        assert_eq!(mapped.text.as_deref(), Some("fixture text"));
        assert_eq!(mapped.attachments[0].kind, AttachmentKind::Video);
        assert_eq!(
            mapped.attachments[0].telegram_file_id.as_deref(),
            Some("456")
        );
        assert_eq!(
            mapped.attachments[0].mime_type.as_deref(),
            Some("video/mp4")
        );
        assert_eq!(mapped.attachments[0].size, Some(2048));
    }

    #[test]
    fn raw_service_message_fixture_is_preserved_as_a_message_without_fake_text() {
        use grammers_client::tl::{enums::Peer, types};

        let service = types::MessageService {
            out: false,
            mentioned: false,
            media_unread: false,
            reactions_are_possible: false,
            silent: false,
            post: false,
            legacy: false,
            id: 55,
            from_id: Some(Peer::User(types::PeerUser { user_id: 5 })),
            peer_id: Peer::Chat(types::PeerChat { chat_id: 6 }),
            saved_peer_id: None,
            reply_to: None,
            date: 1_700_000_001,
            action: grammers_client::tl::enums::MessageAction::PinMessage,
            reactions: None,
            ttl_period: None,
        };
        let mapped = map_raw_message(&service.into(), Utc::now()).unwrap();

        assert_eq!(mapped.id.get(), 55);
        assert_eq!(mapped.chat_id.get(), -6);
        assert_eq!(mapped.sender_id.unwrap().get(), 5);
        assert_eq!(mapped.text, None);
        assert!(mapped.attachments.is_empty());
    }
}

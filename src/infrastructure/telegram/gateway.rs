use async_trait::async_trait;
use chrono::Utc;
use grammers_client::{
    message::Message as TelegramMessage,
    peer::{Dialog, Peer},
};

use crate::{
    application::{
        HistoryBoundary, HistoryPage, IngestRecord, MessageSource, PageSize, TelegramError,
        TelegramGateway,
    },
    domain::{Chat, ChatId, ForumTopic, MessageEvent, MessageId, SenderId},
};

use super::{
    mapper::{self, MappingError},
    session::{ResolvePeerError, TelegramAdapter},
};

mod topics;

#[async_trait]
impl TelegramGateway for TelegramAdapter {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        let mut dialogs = self.client().iter_dialogs();
        let mut chats = Vec::new();
        while let Some(dialog) = dialogs.next().await.map_err(map_invocation_error)? {
            chats.push(map_dialog(&dialog).map_err(map_mapping_error)?);
        }
        Ok(chats)
    }

    async fn list_topics(&self, chat_id: ChatId) -> Result<Vec<ForumTopic>, TelegramError> {
        let (peer, peer_ref) = self
            .resolve_chat(chat_id.get())
            .await
            .map_err(map_resolve_error)?;
        // An inaccessible group stub has no forum flag. It must not invalidate the catalog.
        if matches!(&peer, Peer::Group(group) if matches!(group.raw,
            grammers_client::tl::enums::Chat::Empty(_) | grammers_client::tl::enums::Chat::Forbidden(_)
                | grammers_client::tl::enums::Chat::ChannelForbidden(_)))
        {
            return Err(TelegramError::Unavailable(
                "Telegram group metadata is unavailable".into(),
            ));
        }
        if !mapper::is_forum(&peer) {
            return Ok(Vec::new());
        }
        topics::list(&self.client(), peer_ref).await
    }

    async fn fetch_topic_history(
        &self,
        chat_id: ChatId,
        topic_id: MessageId,
        before: Option<MessageId>,
        page_size: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        let (peer, peer_ref) = self
            .resolve_chat(chat_id.get())
            .await
            .map_err(map_resolve_error)?;
        if !mapper::is_forum(&peer) {
            return Err(TelegramError::Unavailable(
                "chat is not a Telegram forum".into(),
            ));
        }
        let (messages, exhausted) =
            topics::replies(&self.client(), peer_ref, topic_id, before, page_size).await?;
        history_page(
            &peer,
            &messages,
            self.account_id(),
            Some(topic_id),
            false,
            exhausted,
        )
    }

    async fn fetch_history(
        &self,
        chat_id: ChatId,
        boundary: HistoryBoundary,
        page_size: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        boundary.validate().map_err(|error| {
            TelegramError::Unavailable(format!("invalid history boundary: {error}"))
        })?;

        let (peer, peer_ref) = self
            .resolve_chat(chat_id.get())
            .await
            .map_err(map_resolve_error)?;
        let after = boundary.after_message_id;
        let before = boundary.before_message_id;
        let offset_id = before.or(after).map(|id| id.get() as i32).unwrap_or(0);
        let reverse = after.is_some();

        // Grammers' offset ID is documented as exclusive. Read one extra item to distinguish a
        // complete page from an exhausted history without changing the caller's page size.
        let requested = usize::from(page_size.get()) + 1;
        let mut iterator = self
            .client()
            .iter_messages(peer_ref)
            .offset_id(offset_id)
            .reverse(reverse)
            .limit(requested);
        let mut messages = Vec::with_capacity(requested);
        while let Some(message) = iterator.next().await.map_err(map_invocation_error)? {
            messages.push(message);
        }
        messages.retain(|message| is_inside_exclusive_boundary(message.id(), before, after));

        let exhausted = messages.len() <= usize::from(page_size.get());
        messages.truncate(usize::from(page_size.get()));
        history_page(
            &peer,
            &messages,
            self.account_id(),
            None,
            reverse,
            exhausted,
        )
    }
}

fn history_page(
    peer: &Peer,
    messages: &[TelegramMessage],
    account: Option<SenderId>,
    thread: Option<MessageId>,
    reverse: bool,
    exhausted: bool,
) -> Result<HistoryPage, TelegramError> {
    let chat = mapper::map_chat(peer).map_err(map_mapping_error)?;
    let known_forum = mapper::is_forum(peer);
    let collected_at = Utc::now();
    let mut senders = Vec::new();
    let mut records = Vec::with_capacity(messages.len());
    for message in messages {
        // Deleted/inaccessible message stubs still advance the Telegram page cursor.
        if matches!(message.raw, grammers_client::tl::enums::Message::Empty(_)) {
            continue;
        }
        if let Some(sender_id) = message.sender_id() {
            let sender =
                mapper::map_sender(sender_id, message.sender()).map_err(map_mapping_error)?;
            if !senders
                .iter()
                .any(|known: &crate::domain::Sender| known.id == sender.id)
            {
                senders.push(sender);
            }
        }
        let mut mapped = mapper::map_message(message, collected_at, account, known_forum)
            .map_err(map_mapping_error)?;
        // getReplies selects the topic server-side, including when the header is absent.
        mapped.topic_id = thread.or(mapped.topic_id);
        records.push(IngestRecord {
            event: MessageEvent::Created(mapped),
            source: MessageSource::History,
        });
    }

    let last_id = messages
        .last()
        .map(|message| MessageId::new(i64::from(message.id())))
        .transpose()
        .map_err(|error| map_mapping_error(error.into()))?;
    Ok(HistoryPage {
        chats: vec![chat],
        senders,
        records,
        next_before_message_id: if reverse { None } else { last_id },
        next_after_message_id: if reverse { last_id } else { None },
        exhausted,
    })
}

fn is_inside_exclusive_boundary(
    message_id: i32,
    before: Option<MessageId>,
    after: Option<MessageId>,
) -> bool {
    before.is_none_or(|boundary| i64::from(message_id) < boundary.get())
        && after.is_none_or(|boundary| i64::from(message_id) > boundary.get())
}

fn map_dialog(dialog: &Dialog) -> Result<Chat, MappingError> {
    mapper::map_chat(dialog.peer())
}

fn map_mapping_error(error: MappingError) -> TelegramError {
    TelegramError::Unavailable(error.to_string())
}

fn map_resolve_error(error: ResolvePeerError) -> TelegramError {
    match error {
        ResolvePeerError::Telegram(error) => map_invocation_error(error),
        other => TelegramError::Unavailable(other.to_string()),
    }
}

fn map_invocation_error(error: grammers_client::InvocationError) -> TelegramError {
    use grammers_client::InvocationError;

    match error {
        InvocationError::Rpc(ref rpc) if rpc.name == "FLOOD_WAIT" => rpc
            .value
            .map(|seconds| TelegramError::FloodWait {
                retry_after_seconds: u64::from(seconds),
            })
            .unwrap_or_else(|| {
                TelegramError::Unavailable("Telegram returned FLOOD_WAIT without a duration".into())
            }),
        InvocationError::Rpc(ref rpc)
            if matches!(
                rpc.name.as_str(),
                "AUTH_KEY_UNREGISTERED" | "SESSION_REVOKED" | "USER_DEACTIVATED"
            ) =>
        {
            TelegramError::Unauthorized
        }
        other => TelegramError::Unavailable(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_wait_remains_a_typed_caller_action() {
        let mapped = map_invocation_error(grammers_client::InvocationError::Rpc(
            grammers_client::sender::RpcError {
                code: 420,
                name: "FLOOD_WAIT".into(),
                value: Some(29),
                caused_by: None,
            },
        ));
        assert!(matches!(
            mapped,
            TelegramError::FloodWait {
                retry_after_seconds: 29
            }
        ));
    }

    #[test]
    fn history_cursor_is_exclusive_in_both_directions() {
        let cursor = MessageId::new(10).unwrap();
        assert!(!is_inside_exclusive_boundary(10, Some(cursor), None));
        assert!(is_inside_exclusive_boundary(9, Some(cursor), None));
        assert!(!is_inside_exclusive_boundary(10, None, Some(cursor)));
        assert!(is_inside_exclusive_boundary(11, None, Some(cursor)));
    }
}

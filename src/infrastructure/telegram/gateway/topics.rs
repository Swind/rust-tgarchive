use std::collections::HashSet;

use grammers_client::{Client, message::Message as TelegramMessage, tl};
use grammers_session::types::PeerRef;

use crate::{
    application::{PageSize, TelegramError},
    domain::{ForumTopic, MessageId},
};

use super::{map_invocation_error, map_mapping_error, mapper::GENERAL_TOPIC_ID};

const TELEGRAM_PAGE_LIMIT: i32 = 100;

pub(super) async fn list(client: &Client, peer: PeerRef) -> Result<Vec<ForumTopic>, TelegramError> {
    let mut topics = Vec::new();
    let mut seen = HashSet::new();
    let mut request = tl::functions::messages::GetForumTopics {
        peer: peer.into(),
        q: None,
        offset_date: 0,
        offset_id: 0,
        offset_topic: 0,
        limit: TELEGRAM_PAGE_LIMIT,
    };
    loop {
        let tl::enums::messages::ForumTopics::Topics(page) = client
            .invoke(&request)
            .await
            .map_err(map_invocation_error)?;
        if !append_page(page, &mut topics, &mut seen, &mut request)? {
            break;
        }
    }
    if !topics
        .iter()
        .any(|topic| topic.id.get() == i64::from(GENERAL_TOPIC_ID))
    {
        // General is permanent, even when hidden/empty and omitted from the listing. Ask for
        // its metadata explicitly so a renamed General retains its configured title.
        let tl::enums::messages::ForumTopics::Topics(page) = client
            .invoke(&tl::functions::messages::GetForumTopicsById {
                peer: peer.into(),
                topics: vec![GENERAL_TOPIC_ID],
            })
            .await
            .map_err(map_invocation_error)?;
        let title = page.topics.into_iter().find_map(|topic| match topic {
            tl::enums::ForumTopic::Topic(topic) if topic.id == GENERAL_TOPIC_ID => {
                Some(topic.title)
            }
            _ => None,
        });
        topics.push(ForumTopic {
            id: message_id(GENERAL_TOPIC_ID)?,
            title: title.unwrap_or_else(|| "General".into()),
        });
    }
    Ok(topics)
}

/// A short page is not an end marker: getForumTopics can return fewer than `limit`.
fn append_page(
    page: tl::types::messages::ForumTopics,
    topics: &mut Vec<ForumTopic>,
    seen: &mut HashSet<i32>,
    request: &mut tl::functions::messages::GetForumTopics,
) -> Result<bool, TelegramError> {
    if page.topics.is_empty() {
        return Ok(false);
    }
    let previous_count = seen.len();
    let mut last_topic = None;
    for entry in page.topics {
        let new = seen.insert(entry.id());
        if let tl::enums::ForumTopic::Topic(topic) = entry {
            last_topic = Some((topic.date, topic.top_message, topic.id));
            if new {
                topics.push(ForumTopic {
                    id: message_id(topic.id)?,
                    title: topic.title,
                });
            }
        }
    }
    if seen.len() >= page.count as usize {
        return Ok(false);
    }
    let (date, top_message, id) = last_topic
        .filter(|_| seen.len() > previous_count)
        .ok_or_else(|| TelegramError::Unavailable("Telegram topic list did not advance".into()))?;
    request.offset_topic = id;
    request.offset_id = top_message;
    request.offset_date = if page.order_by_create_date || top_message == 0 {
        date
    } else {
        page.messages
            .iter()
            .find_map(|message| match message {
                tl::enums::Message::Message(m) if m.id == top_message => Some(m.date),
                tl::enums::Message::Service(m) if m.id == top_message => Some(m.date),
                _ => None,
            })
            .ok_or_else(|| {
                TelegramError::Unavailable("missing Telegram topic pagination message".into())
            })?
    };
    Ok(true)
}

pub(super) async fn replies(
    client: &Client,
    peer: PeerRef,
    topic_id: MessageId,
    before: Option<MessageId>,
    page_size: PageSize,
) -> Result<(Vec<TelegramMessage>, bool), TelegramError> {
    use tl::enums::messages::Messages;

    let root = topic_id.get() as i32;
    let offset_id = before.map_or(0, |id| id.get() as i32);
    let response = client
        .invoke(&tl::functions::messages::GetReplies {
            peer: peer.into(),
            msg_id: root,
            offset_id,
            offset_date: 0,
            add_offset: 0,
            limit: i32::from(page_size.get()).min(TELEGRAM_PAGE_LIMIT),
            max_id: offset_id,
            min_id: 0,
            hash: 0,
        })
        .await
        .map_err(map_invocation_error)?;
    let (messages, users, chats) = match response {
        Messages::Messages(m) => (m.messages, m.users, m.chats),
        Messages::Slice(m) => (m.messages, m.users, m.chats),
        Messages::ChannelMessages(m) => (m.messages, m.users, m.chats),
        Messages::NotModified(_) => {
            return Err(TelegramError::Unavailable(
                "unexpected cached topic history".into(),
            ));
        }
    };
    if messages.is_empty() {
        // getReplies lists the replies, not necessarily the service message that created the
        // topic. Include that root once, as the last record in the walk.
        let root_message = if before.is_none_or(|cursor| cursor.get() > topic_id.get()) {
            client
                .get_messages_by_id(peer, &[root])
                .await
                .map_err(map_invocation_error)?
                .into_iter()
                .flatten()
                .collect()
        } else {
            Vec::new()
        };
        return Ok((root_message, true));
    }
    let peers = client.build_peer_map(users, chats).await;
    let messages: Vec<_> = messages
        .into_iter()
        .filter(|message| offset_id == 0 || message.id() < offset_id)
        .map(|message| TelegramMessage::from_raw(client, message, Some(peer), peers.handle()))
        .collect();
    let last = messages.last().ok_or_else(|| {
        TelegramError::Unavailable("Telegram topic history did not advance".into())
    })?;
    let exhausted = last.id() == root;
    Ok((messages, exhausted))
}

fn message_id(id: i32) -> Result<MessageId, TelegramError> {
    MessageId::new(i64::from(id)).map_err(|error| map_mapping_error(error.into()))
}

#[cfg(test)]
mod tests;

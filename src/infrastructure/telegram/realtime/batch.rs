use std::collections::HashSet;

use chrono::Utc;
use grammers_client::{Client, peer::PeerMap, tl, update::Update};
use grammers_session::updates::State;

use crate::{
    application::{AccountDeletion, IngestBatch, IngestRecord, MessageSource},
    domain::{Chat, MessageEvent, MessageId},
};

use super::{chat_kind, mapper};

#[derive(Default)]
pub(super) struct NormalizedBatch {
    pub(super) chats: Vec<Chat>,
    pub(super) senders: Vec<crate::domain::Sender>,
    pub(super) records: Vec<IngestRecord>,
    pub(super) account_deletions: Vec<AccountDeletion>,
    pub(super) seen_chats: HashSet<i64>,
    pub(super) seen_senders: HashSet<i64>,
    pub(super) account: Option<crate::domain::SenderId>,
}

impl NormalizedBatch {
    pub(super) fn with_account(account: Option<crate::domain::SenderId>) -> Self {
        Self {
            account,
            ..Self::default()
        }
    }

    fn include_chat(&mut self, chat: Chat) {
        if self.seen_chats.insert(chat.id.get()) {
            self.chats.push(chat);
        }
    }

    fn include_sender(&mut self, sender: crate::domain::Sender) {
        if self.seen_senders.insert(sender.id.get()) {
            self.senders.push(sender);
        }
    }

    pub(super) fn add_update(
        &mut self,
        client: &Client,
        raw: tl::enums::Update,
        state: State,
        peers: PeerMap,
    ) -> Result<(), mapper::MappingError> {
        let update = Update::from_raw(client, raw, state, peers);
        let collected_at = Utc::now();
        match update {
            Update::NewMessage(message) => self.add_message(&message, false, collected_at)?,
            Update::MessageEdited(message) => self.add_message(&message, true, collected_at)?,
            Update::MessageDeleted(deletion) => {
                self.add_deletion(&deletion.raw, collected_at)?;
            }
            // This archive stores messages; acknowledge other update kinds without inventing
            // message records. Their pts still belongs to this fully processed stream batch.
            _ => {}
        }
        Ok(())
    }

    pub(super) fn add_deletion(
        &mut self,
        raw: &tl::enums::Update,
        deleted_at: chrono::DateTime<Utc>,
    ) -> Result<(), mapper::MappingError> {
        match raw {
            tl::enums::Update::DeleteMessages(update) => {
                self.account_deletions.extend(
                    update
                        .messages
                        .iter()
                        .map(|id| {
                            MessageId::new(i64::from(*id)).map(|message_id| AccountDeletion {
                                message_id,
                                deleted_at,
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                );
            }
            tl::enums::Update::DeleteChannelMessages(update) => {
                let peer_id = grammers_session::types::PeerId::channel(update.channel_id)
                    .ok_or(mapper::MappingError::InvalidPeerId)?;
                let chat_id = mapper::chat_id(peer_id)?;
                for id in &update.messages {
                    self.records.push(IngestRecord {
                        event: MessageEvent::Deleted {
                            chat_id,
                            message_id: MessageId::new(i64::from(*id))?,
                            deleted_at,
                        },
                        source: MessageSource::Realtime,
                    });
                }
            }
            _ => unreachable!("only deletion updates reach add_deletion"),
        }
        Ok(())
    }

    fn add_message(
        &mut self,
        message: &grammers_client::update::Message,
        edited: bool,
        collected_at: chrono::DateTime<Utc>,
    ) -> Result<(), mapper::MappingError> {
        let chat_id = mapper::chat_id(message.peer_id())?;
        if let Some(peer) = message.peer() {
            self.include_chat(mapper::map_chat(peer)?);
        } else {
            self.include_chat(Chat {
                id: chat_id,
                kind: chat_kind(message.peer_id()),
                title: None,
                username: None,
                tracked: false,
            });
        }
        if let Some(sender_id) = message.sender_id() {
            self.include_sender(mapper::map_sender(sender_id, message.sender())?);
        }
        let mapped = mapper::map_message(message, collected_at, self.account)?;
        self.records.push(IngestRecord {
            event: if edited {
                MessageEvent::Updated(mapped)
            } else {
                MessageEvent::Created(mapped)
            },
            source: MessageSource::Realtime,
        });
        Ok(())
    }

    pub(super) fn into_ingest_batch(self) -> IngestBatch {
        IngestBatch {
            chats: self.chats,
            senders: self.senders,
            records: self.records,
            account_deletions: self.account_deletions,
            ..IngestBatch::default()
        }
    }
}

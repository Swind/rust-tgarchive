//! In-process archive change notifications; invalidation hints only, never a source of truth.

use std::collections::BTreeSet;

use serde::Serialize;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::IngestBatch;
use crate::domain::MessageEvent;

pub const EVENT_CAPACITY: usize = 256;

/// Chats touched by one committed batch; `global` means any chat may have changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ArchiveChange {
    pub chat_ids: Vec<i64>,
    pub global: bool,
}

impl ArchiveChange {
    /// Collect affected chats; call before the batch is consumed by the writer.
    pub fn from_batch(batch: &IngestBatch) -> Option<Self> {
        let mut ids = BTreeSet::new();
        ids.extend(batch.chats.iter().map(|chat| chat.id.get()));
        for record in &batch.records {
            ids.insert(match &record.event {
                MessageEvent::Created(message) | MessageEvent::Updated(message) => {
                    message.chat_id.get()
                }
                MessageEvent::Deleted { chat_id, .. } => chat_id.get(),
            });
        }
        let global = !batch.account_deletions.is_empty();
        (global || !ids.is_empty()).then(|| Self {
            chat_ids: ids.into_iter().collect(),
            global,
        })
    }
}

#[derive(Clone)]
pub struct ArchiveEvents {
    sender: broadcast::Sender<ArchiveChange>,
    shutdown: CancellationToken,
}

impl ArchiveEvents {
    pub fn new(capacity: usize) -> Self {
        Self {
            sender: broadcast::channel(capacity.max(1)).0,
            shutdown: CancellationToken::new(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ArchiveChange> {
        self.sender.subscribe()
    }

    pub fn publish(&self, change: ArchiveChange) {
        let _ = self.sender.send(change);
    }

    /// Ends every SSE stream so HTTP graceful shutdown does not wait on them.
    pub fn close(&self) {
        self.shutdown.cancel();
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }
}

impl Default for ArchiveEvents {
    fn default() -> Self {
        Self::new(EVENT_CAPACITY)
    }
}

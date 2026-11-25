use super::{
    ApplicationError, CancellationToken, IngestBatch, MessageSource, SyncEngine, cursor_advances,
    sanitize_reason, wait_or_cancel,
};
use crate::domain::{Chat, ChatId, ChatKind, MessageId, TopicBackfill};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use tokio::time::Instant;

const POLL: Duration = Duration::from_secs(15);
const REFRESH: Duration = Duration::from_secs(300);
type DeferredTopics = HashMap<(ChatId, MessageId, i64), Instant>;

fn candidate(chat: &Chat) -> bool {
    chat.tracked && chat.kind == ChatKind::Supergroup
}

impl SyncEngine {
    async fn discover_topics(
        &self,
        chat: ChatId,
        cancel: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        let topics = self
            .request_with_retry(cancel, || self.gateway.list_topics(chat))
            .await?;
        self.chats.save_topics(chat, topics).await?;
        Ok(())
    }

    /// Explicit sync must also work offline from the server (including an already-complete
    /// chat checkpoint). Topic cursors are independent of normal history and --refetch.
    pub(super) async fn sync_chat_topics(
        &self,
        id: ChatId,
        cancel: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        if !self.chats.get(id).await?.as_ref().is_some_and(candidate) {
            return Ok(());
        }
        self.discover_topics(id, cancel).await?;
        let mut deferred = HashMap::new();
        while self
            .backfill_topic_round(Some(id), &mut deferred, cancel)
            .await?
        {}
        Ok(())
    }

    /// Runs alongside (not before) live ingestion. Refresh newly tracked groups on the next
    /// poll and periodically rediscover titles/new topics even in a quiet Telegram session.
    pub(crate) async fn maintain_topics(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        let mut discovery_due = HashMap::new();
        let mut deferred = HashMap::new();
        let mut next_scan = Instant::now();
        while !cancel.is_cancelled() {
            let round = async {
                if Instant::now() >= next_scan {
                    self.refresh_due_topics(&mut discovery_due, cancel).await?;
                    next_scan = Instant::now() + POLL;
                }
                self.backfill_topic_round(None, &mut deferred, cancel).await
            }
            .await;
            let delay = match round {
                Ok(true) => continue,
                Ok(false) => POLL,
                Err(_) if cancel.is_cancelled() => break,
                Err(ApplicationError::TelegramFloodWait {
                    retry_after_seconds,
                }) => {
                    // Unlike an explicit job, a daemon will retry automatically. Do not
                    // turn an above-ceiling FLOOD_WAIT into a new request every poll.
                    Duration::from_secs(retry_after_seconds).max(POLL)
                }
                Err(error) => return Err(error),
            };
            wait_or_cancel(cancel, delay).await;
        }
        Ok(())
    }

    async fn refresh_due_topics(
        &self,
        due: &mut HashMap<ChatId, Instant>,
        cancel: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        let tracked: HashSet<_> = self
            .chats
            .list()
            .await?
            .into_iter()
            .filter(candidate)
            .map(|chat| chat.id)
            .collect();
        due.retain(|id, _| tracked.contains(id));
        for id in tracked {
            if due.get(&id).is_some_and(|at| *at > Instant::now()) {
                continue;
            }
            let delay = match self.discover_topics(id, cancel).await {
                Ok(()) => REFRESH,
                Err(error @ ApplicationError::TelegramUnavailable(_)) => {
                    tracing::warn!(chat_id = id.get(), reason = %sanitize_reason(&error.to_string()), "forum topic discovery failed; retrying later");
                    POLL
                }
                Err(error) => return Err(error),
            };
            due.insert(id, Instant::now() + delay);
        }
        Ok(())
    }

    /// One page per pending topic so a deep history cannot starve other forums. Re-read
    /// preference and cursor state between rounds; SQLite also checks it at commit time.
    async fn backfill_topic_round(
        &self,
        only: Option<ChatId>,
        deferred: &mut DeferredTopics,
        cancel: &CancellationToken,
    ) -> Result<bool, ApplicationError> {
        let _round = tokio::select! {
            _ = cancel.cancelled() => return Err(ApplicationError::Conflict),
            guard = self.topic_round.lock() => guard,
        };
        let mut advanced = false;
        deferred.retain(|_, until| *until > Instant::now());
        for work in self.repository.pending_topic_backfills().await? {
            let key = (work.chat_id, work.topic_id, work.generation);
            if only.is_some_and(|id| id != work.chat_id) || deferred.contains_key(&key) {
                continue;
            }
            match self.backfill_topic_page(work, cancel).await {
                Ok(()) => advanced = true,
                Err(error @ ApplicationError::TelegramUnavailable(_)) if only.is_none() => {
                    tracing::warn!(chat_id = work.chat_id.get(), topic_id = work.topic_id.get(), reason = %sanitize_reason(&error.to_string()), "forum topic backfill failed; retrying later");
                    deferred.insert(key, Instant::now() + REFRESH);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(advanced)
    }

    async fn backfill_topic_page(
        &self,
        work: TopicBackfill,
        cancel: &CancellationToken,
    ) -> Result<(), ApplicationError> {
        let mut page = self
            .request_with_retry(cancel, || {
                self.gateway.fetch_topic_history(
                    work.chat_id,
                    work.topic_id,
                    work.before_id,
                    self.page_size,
                )
            })
            .await?;
        let next = if page.exhausted {
            None
        } else {
            let next = page.next_before_message_id;
            if !cursor_advances(work.before_id, next) {
                return Err(ApplicationError::TelegramUnavailable(
                    "topic history page did not advance its exclusive cursor".into(),
                ));
            }
            next
        };
        // This also fills legacy NULL topic IDs; never replace newer text or revive tombstones.
        for record in &mut page.records {
            record.source = MessageSource::Refetch;
        }
        self.sink
            .submit(IngestBatch {
                chats: page.chats,
                senders: page.senders,
                records: page.records,
                topic_backfill: Some((work, next)),
                ..Default::default()
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

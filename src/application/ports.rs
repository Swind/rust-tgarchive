use super::*;
use crate::domain::{Chat, ChatId, MessageId, SenderId};
use async_trait::async_trait;

#[async_trait]
pub trait MessageRepository: Send + Sync {
    async fn get(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        include_deleted: bool,
    ) -> Result<Option<MessageView>, RepositoryError>;
    async fn list(&self, query: ListMessagesQuery) -> Result<MessagePage, RepositoryError>;
    async fn search(&self, query: SearchMessagesQuery) -> Result<MessagePage, RepositoryError>;

    /// State of the full-text index. Default: ready and empty.
    async fn search_index_status(&self) -> Result<SearchIndexStatus, RepositoryError> {
        Ok(SearchIndexStatus {
            version: 0,
            state: SearchIndexState::Ready,
            indexed: 0,
            total: 0,
        })
    }

    /// Senders with messages in the chat, most active first. Default: none.
    async fn list_senders(
        &self,
        _chat_id: ChatId,
        _limit: PageSize,
    ) -> Result<Vec<SenderSummary>, RepositoryError> {
        Ok(Vec::new())
    }

    /// The Telegram user the archive is bound to. Default: unbound.
    async fn bound_account(&self) -> Result<Option<SenderId>, RepositoryError> {
        Ok(None)
    }

    /// Senders with archive-wide aggregates. Default: none.
    async fn search_senders(&self, _query: SenderQuery) -> Result<SenderPage, RepositoryError> {
        Ok(SenderPage {
            items: Vec::new(),
            next_offset: None,
        })
    }

    /// `None` when nothing is known about the sender. Default: none.
    async fn sender_detail(
        &self,
        _id: SenderId,
        _include_deleted: bool,
    ) -> Result<Option<SenderDetail>, RepositoryError> {
        Ok(None)
    }

    /// `None` when the anchor does not exist (or is deleted and `include_deleted` is false).
    async fn context(
        &self,
        query: MessageContextQuery,
    ) -> Result<Option<MessageContext>, RepositoryError> {
        let Some(anchor) = self
            .get(query.chat_id, query.message_id, query.include_deleted)
            .await?
        else {
            return Ok(None);
        };
        let cursor = MessageCursor {
            timestamp: anchor.timestamp,
            chat_id: query.chat_id,
            message_id: query.message_id,
        };
        let filters = MessageFilters {
            chat_id: Some(query.chat_id),
            sender_id: None,
            post_author: None,
            time_range: TimeRange {
                from: None,
                to: None,
            },
            include_deleted: query.include_deleted,
            exclude_bots: false,
        };
        let empty = || MessagePage {
            items: Vec::new(),
            has_more: false,
            next_cursor: None,
            next_offset: None,
        };
        let page_size = |count: u16| {
            PageSize::new(count).map_err(|error| RepositoryError::InvalidData(error.to_string()))
        };
        let mut older = empty();
        if query.before > 0 {
            older = self
                .list(ListMessagesQuery {
                    filters: filters.clone(),
                    before: Some(cursor.clone()),
                    after: None,
                    page_size: page_size(query.before)?,
                })
                .await?;
            older.items.reverse();
        }
        let mut newer = empty();
        if query.after > 0 {
            newer = self
                .list(ListMessagesQuery {
                    filters,
                    before: None,
                    after: Some(cursor),
                    page_size: page_size(query.after)?,
                })
                .await?;
            // `after` pages are newest-first like every list page.
            newer.items.reverse();
        }
        Ok(Some(MessageContext {
            anchor,
            before: older.items,
            after: newer.items,
            has_more_before: older.has_more,
            has_more_after: newer.has_more,
            before_cursor: older.next_cursor,
            after_cursor: newer.next_cursor,
        }))
    }
}

#[async_trait]
pub trait ChatRepository: Send + Sync {
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError>;
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError>;
    /// Chats with aggregate message/sync stats. Default: empty stats.
    async fn list_with_stats(&self, _sort: ChatSort) -> Result<Vec<ChatSummary>, RepositoryError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .map(|chat| ChatSummary {
                chat,
                stats: ChatStats::default(),
            })
            .collect())
    }
    async fn get_with_stats(&self, id: ChatId) -> Result<Option<ChatSummary>, RepositoryError> {
        Ok(self.get(id).await?.map(|chat| ChatSummary {
            chat,
            stats: ChatStats::default(),
        }))
    }
    /// Metadata-only upsert: must never change the tracking flag.
    async fn save_refresh(&self, chats: Vec<Chat>) -> Result<(), RepositoryError>;
    /// Idempotently sets the tracking flag; `None` when the chat is unknown.
    async fn set_tracked(&self, id: ChatId, tracked: bool)
    -> Result<Option<Chat>, RepositoryError>;
}

/// Read side of the opt-in collection scope, consulted by realtime ingestion for every batch so
/// tracking changes apply without a restart.
#[async_trait]
pub trait TrackingScope: Send + Sync {
    async fn tracked_chat_ids(&self) -> Result<std::collections::HashSet<ChatId>, RepositoryError>;
    /// Of `ids`, those already archived as non-channel (common-namespace) messages of a tracked chat.
    async fn archived_common_message_ids(
        &self,
        ids: &[MessageId],
    ) -> Result<std::collections::HashSet<MessageId>, RepositoryError>;
}

#[async_trait]
pub trait ArchiveWriter: Send + Sync {
    async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError>;
}

#[async_trait]
pub trait TelegramGateway: Send + Sync {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError>;
    async fn fetch_history(
        &self,
        chat_id: ChatId,
        boundary: HistoryBoundary,
        page_size: PageSize,
    ) -> Result<HistoryPage, TelegramError>;
}

#[async_trait]
pub trait SyncRepository: Send + Sync {
    async fn get_checkpoint(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<ChatCheckpoint>, RepositoryError>;
    /// State of the `--refetch` walk of a chat (none = no walk in progress).
    async fn get_refetch_checkpoint(
        &self,
        _chat_id: ChatId,
    ) -> Result<RefetchCheckpoint, RepositoryError> {
        Ok(RefetchCheckpoint::default())
    }
    /// Newest message ID archived for the chat; used to baseline catch-up without a gap.
    async fn newest_archived_id(
        &self,
        _chat_id: ChatId,
    ) -> Result<Option<MessageId>, RepositoryError> {
        Ok(None)
    }
    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError>;
    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError>;
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError>;
    /// Deletions whose chat could not be resolved (common-message tombstones).
    async fn unresolved_deletions(&self) -> Result<u64, RepositoryError> {
        Ok(0)
    }
    async fn list_chat_progress(
        &self,
        job_id: &str,
    ) -> Result<Vec<SyncChatProgress>, RepositoryError>;
    async fn recover_interrupted(&self) -> Result<(), RepositoryError>;
}

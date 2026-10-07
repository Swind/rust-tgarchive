use super::messages::MAX_SEARCH_LENGTH;
use super::*;
use crate::domain::{Chat, ChatId};
use crate::domain::{ChatKind, MessageId};
use async_trait::async_trait;
use chrono::DateTime;
use std::sync::Arc;

fn filters() -> MessageFilters {
    MessageFilters {
        chat_id: None,
        sender_id: None,
        post_author: None,
        time_range: TimeRange::new(None, None).unwrap(),
        include_deleted: false,
        exclude_bots: false,
    }
}

#[test]
fn page_size_and_query_boundaries_are_validated() {
    assert!(PageSize::new(1).is_ok());
    assert!(PageSize::new(PageSize::MAX).is_ok());
    assert!(PageSize::new(0).is_err());
    assert!(PageSize::new(PageSize::MAX + 1).is_err());
    assert!(serde_json::from_str::<PageSize>("0").is_err());
    assert!(serde_json::from_str::<PageSize>("1001").is_err());
    let cursor = MessageCursor {
        timestamp: DateTime::from_timestamp(10, 0).unwrap(),
        chat_id: ChatId::from_telegram(ChatKind::Private, 3).unwrap(),
        message_id: MessageId::new(2).unwrap(),
    };
    let query = ListMessagesQuery {
        filters: filters(),
        before: Some(cursor.clone()),
        after: Some(cursor),
        page_size: PageSize::DEFAULT,
    };
    assert_eq!(query.validate(), Err(ValidationError::ConflictingCursors));
    assert_eq!(
        TimeRange::new(
            Some(DateTime::from_timestamp(2, 0).unwrap()),
            Some(DateTime::from_timestamp(2, 0).unwrap())
        ),
        Err(ValidationError::InvalidTimeRange)
    );
    let invalid_filters = MessageFilters {
        chat_id: None,
        sender_id: None,
        post_author: None,
        time_range: TimeRange {
            from: Some(DateTime::from_timestamp(2, 0).unwrap()),
            to: Some(DateTime::from_timestamp(1, 0).unwrap()),
        },
        include_deleted: false,
        exclude_bots: false,
    };
    assert_eq!(
        ListMessagesQuery {
            filters: invalid_filters,
            before: None,
            after: None,
            page_size: PageSize::DEFAULT
        }
        .validate(),
        Err(ValidationError::InvalidTimeRange)
    );
    let too_long = SearchMessagesQuery {
        text: "a".repeat(MAX_SEARCH_LENGTH + 1),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::DEFAULT,
        sort: SearchSort::Relevance,
        offset: 0,
    };
    assert_eq!(too_long.validate(), Err(ValidationError::SearchTooLong));
    let punctuation = SearchMessagesQuery {
        text: " ！？\" * ".into(),
        ..too_long
    };
    assert_eq!(punctuation.validate(), Err(ValidationError::EmptySearch));
}

#[test]
fn compound_cursor_orders_same_timestamp_without_collisions() {
    let timestamp = DateTime::from_timestamp(10, 0).unwrap();
    let left = MessageCursor {
        timestamp,
        chat_id: ChatId::from_telegram(ChatKind::Private, 4).unwrap(),
        message_id: MessageId::new(1).unwrap(),
    };
    let right = MessageCursor {
        timestamp,
        chat_id: ChatId::from_telegram(ChatKind::Private, 3).unwrap(),
        message_id: MessageId::new(1).unwrap(),
    };
    assert_eq!(left.compare_key(&right), std::cmp::Ordering::Greater);
}

struct FakePorts;

#[async_trait]
impl MessageRepository for FakePorts {
    async fn get(
        &self,
        _: ChatId,
        _: MessageId,
        _: bool,
    ) -> Result<Option<MessageView>, RepositoryError> {
        Ok(None)
    }
    async fn list(&self, _: ListMessagesQuery) -> Result<MessagePage, RepositoryError> {
        Ok(MessagePage {
            items: vec![],
            has_more: false,
            next_cursor: None,
            next_offset: None,
        })
    }
    async fn search(&self, _: SearchMessagesQuery) -> Result<MessagePage, RepositoryError> {
        Ok(MessagePage {
            items: vec![],
            has_more: false,
            next_cursor: None,
            next_offset: None,
        })
    }
}

#[async_trait]
impl ChatRepository for FakePorts {
    async fn get(&self, _: ChatId) -> Result<Option<Chat>, RepositoryError> {
        Ok(None)
    }
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
        Ok(vec![])
    }
    async fn save_refresh(&self, _: Vec<Chat>) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn set_tracked(&self, _: ChatId, _: bool) -> Result<Option<Chat>, RepositoryError> {
        Ok(None)
    }
}

#[async_trait]
impl ArchiveWriter for FakePorts {
    async fn write_batch(&self, _: IngestBatch) -> Result<(), RepositoryError> {
        Ok(())
    }
}

#[async_trait]
impl TelegramGateway for FakePorts {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        Ok(vec![])
    }
    async fn fetch_history(
        &self,
        _: ChatId,
        _: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        Ok(HistoryPage {
            chats: vec![],
            senders: vec![],
            records: vec![],
            next_before_message_id: None,
            next_after_message_id: None,
            exhausted: true,
        })
    }
}

#[async_trait]
impl SyncRepository for FakePorts {
    async fn get_checkpoint(&self, _: ChatId) -> Result<Option<ChatCheckpoint>, RepositoryError> {
        Ok(None)
    }
    async fn save_job(&self, _: SyncJob) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn get_job(&self, _: &str) -> Result<Option<SyncJob>, RepositoryError> {
        Ok(None)
    }
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        Ok(vec![])
    }
    async fn list_chat_progress(&self, _: &str) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        Ok(vec![])
    }
    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

#[test]
fn fake_implements_all_ports_as_shared_trait_objects() {
    let fake = Arc::new(FakePorts);
    let _: Arc<dyn MessageRepository> = fake.clone();
    let _: Arc<dyn ChatRepository> = fake.clone();
    let _: Arc<dyn ArchiveWriter> = fake.clone();
    let _: Arc<dyn TelegramGateway> = fake.clone();
    let _: Arc<dyn SyncRepository> = fake;
}

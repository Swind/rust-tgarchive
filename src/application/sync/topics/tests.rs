use super::*;
use crate::{
    application::{
        ArchiveWriter, ChatRepository, HistoryBoundary, HistoryPage, IngestRecord,
        MessageRepository, PageSize, SyncRepository, TelegramError, TelegramGateway,
        ingestion_worker,
    },
    domain::{ForumTopic, Message, MessageEvent},
    infrastructure::persistence::sqlite::SqliteStore,
};
use async_trait::async_trait;
use chrono::Utc;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

type ExpectedPage = (Option<MessageId>, Result<HistoryPage, TelegramError>);

struct Gateway {
    topic: MessageId,
    pages: Mutex<VecDeque<ExpectedPage>>,
}

#[async_trait]
impl TelegramGateway for Gateway {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        panic!("a topic walk must not enumerate other chats")
    }

    async fn fetch_history(
        &self,
        _: ChatId,
        _: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        panic!("a topic walk must use scoped history")
    }

    async fn list_topics(&self, _: ChatId) -> Result<Vec<ForumTopic>, TelegramError> {
        Ok(vec![ForumTopic {
            id: self.topic,
            title: "Schedule".into(),
        }])
    }

    async fn fetch_topic_history(
        &self,
        _: ChatId,
        topic: MessageId,
        before: Option<MessageId>,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        assert_eq!(topic, self.topic);
        let (expected, page) = self
            .pages
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected topic fetch");
        assert_eq!(before, expected);
        page
    }
}

fn page(records: Vec<IngestRecord>, next: Option<MessageId>) -> HistoryPage {
    HistoryPage {
        chats: vec![],
        senders: vec![],
        records,
        next_before_message_id: next,
        next_after_message_id: None,
        exhausted: next.is_none(),
    }
}

#[tokio::test]
async fn topic_backfill_resumes_after_flood_and_restarts_only_on_reenable() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteStore::connect(&format!(
            "sqlite://{}",
            dir.path().join("archive.db").display()
        ))
        .await
        .unwrap(),
    );
    let chat = ChatId::from_telegram(ChatKind::Supergroup, 42).unwrap();
    let topic = MessageId::new(10).unwrap();
    let msg = MessageId::new(25).unwrap();
    let cursor = Some(MessageId::new(30).unwrap());
    let now = Utc::now();
    let record = |topic_id, text: &str| IngestRecord {
        source: MessageSource::History,
        event: MessageEvent::Created(Message {
            chat_id: chat,
            id: msg,
            topic_id,
            timestamp: now,
            collected_at: now,
            edited_at: None,
            sender_id: None,
            text: Some(text.into()),
            reply_to: None,
            post_author: None,
            forward: None,
            attachments: vec![],
        }),
    };
    store
        .write_batch(IngestBatch {
            chats: vec![Chat {
                id: chat,
                kind: ChatKind::Supergroup,
                title: None,
                username: None,
                tracked: false,
            }],
            records: vec![record(None, "archived")],
            ..Default::default()
        })
        .await
        .unwrap();
    let gateway = Arc::new(Gateway {
        topic,
        pages: Mutex::new(VecDeque::from([
            (None, Ok(page(vec![], cursor))),
            (
                cursor,
                Err(TelegramError::FloodWait {
                    retry_after_seconds: 3600,
                }),
            ),
            (
                cursor,
                Ok(page(vec![record(Some(topic), "older Telegram text")], None)),
            ),
            (None, Ok(page(vec![], None))),
        ])),
    });
    let (sink, writer) = ingestion_worker::spawn(store.clone(), 2);
    let engine = SyncEngine::new(
        gateway.clone(),
        store.clone(),
        store.clone(),
        sink,
        PageSize::DEFAULT,
    );
    let cancel = CancellationToken::new();
    engine.sync_chat_topics(chat, &cancel).await.unwrap();
    assert!(store.list_topics(chat).await.unwrap().is_empty());
    store.set_tracked(chat, true).await.unwrap();
    let checkpoint = store.get_checkpoint(chat).await.unwrap();
    engine.discover_topics(chat, &cancel).await.unwrap();
    assert!(store.list_topics(chat).await.unwrap()[0].collected);
    store.set_topic_collected(chat, topic, false).await.unwrap();
    store.set_topic_collected(chat, topic, true).await.unwrap();
    assert!(matches!(
        engine.sync_chat_topics(chat, &cancel).await,
        Err(ApplicationError::TelegramFloodWait {
            retry_after_seconds: 3600
        })
    ));
    assert_eq!(
        store.pending_topic_backfills().await.unwrap()[0].before_id,
        cursor
    );

    store.set_tracked(chat, false).await.unwrap();
    assert!(store.pending_topic_backfills().await.unwrap().is_empty());
    store.set_tracked(chat, true).await.unwrap();
    engine.sync_chat_topics(chat, &cancel).await.unwrap();
    let saved = MessageRepository::get(store.as_ref(), chat, msg, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.topic_id, Some(topic));
    assert_eq!(saved.text.as_deref(), Some("archived"));
    assert!(store.pending_topic_backfills().await.unwrap().is_empty());

    store.set_topic_collected(chat, topic, false).await.unwrap();
    engine.sync_chat_topics(chat, &cancel).await.unwrap();
    store.set_topic_collected(chat, topic, true).await.unwrap();
    engine.sync_chat_topics(chat, &cancel).await.unwrap();
    assert!(gateway.pages.lock().unwrap().is_empty());
    assert_eq!(store.get_checkpoint(chat).await.unwrap(), checkpoint);
    drop(engine);
    writer.await.unwrap().unwrap();
    store.close().await;
}

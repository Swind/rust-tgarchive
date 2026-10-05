use chrono::{DateTime, Utc};
use tgarchive::{
    application::{
        AccountDeletion, ArchiveWriter, IngestBatch, IngestRecord, MessageRepository, MessageSource,
    },
    domain::{Attachment, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId, SenderId},
    infrastructure::persistence::sqlite::SqliteStore,
};

async fn store() -> (tempfile::TempDir, SqliteStore) {
    let directory = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", directory.path().join("archive.db").display());
    (directory, SqliteStore::connect(&url).await.unwrap())
}

fn chat(marked_id: i64) -> Chat {
    Chat {
        id: ChatId::from_marked(marked_id).unwrap(),
        kind: if marked_id > 0 {
            ChatKind::Private
        } else if marked_id > -1_000_000_000_000 {
            ChatKind::Group
        } else {
            ChatKind::Channel
        },
        title: None,
        username: None,
        tracked: false,
    }
}

fn message(marked_chat_id: i64, message_id: i64) -> Message {
    let timestamp = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    Message {
        post_author: None,
        forward: None,
        id: MessageId::new(message_id).unwrap(),
        chat_id: ChatId::from_marked(marked_chat_id).unwrap(),
        sender_id: Some(SenderId::from_marked(1).unwrap()),
        timestamp,
        edited_at: None,
        collected_at: timestamp,
        text: Some("fixture".into()),
        reply_to: None,
        attachments: vec![Attachment {
            kind: tgarchive::domain::AttachmentKind::Other,
            telegram_file_id: None,
            mime_type: None,
            file_name: None,
            size: None,
        }],
    }
}

fn insert_messages(messages: Vec<Message>) -> IngestBatch {
    IngestBatch {
        chats: messages.iter().map(|m| chat(m.chat_id.get())).collect(),
        records: messages
            .into_iter()
            .map(|message| IngestRecord {
                event: MessageEvent::Created(message),
                source: MessageSource::History,
            })
            .collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn unknown_common_deletion_is_durable_and_blocks_later_history_without_touching_channel() {
    let (_directory, store) = store().await;
    store
        .bind_telegram_account(SenderId::from_marked(1).unwrap())
        .await
        .unwrap();
    let id = MessageId::new(77).unwrap();
    store
        .write_batch(IngestBatch {
            account_deletions: vec![AccountDeletion {
                message_id: id,
                deleted_at: DateTime::from_timestamp(1_700_000_010, 0).unwrap(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let common_chat = ChatId::from_marked(-21).unwrap();
    let channel_chat = ChatId::from_marked(-1_000_000_000_021).unwrap();
    store
        .write_batch(insert_messages(vec![
            message(common_chat.get(), 77),
            message(channel_chat.get(), 77),
        ]))
        .await
        .unwrap();

    assert!(store.get(common_chat, id, false).await.unwrap().is_none());
    assert!(store.get(channel_chat, id, false).await.unwrap().is_some());
    assert_eq!(store.unresolved_common_deletion_count().await.unwrap(), 1);
}

#[tokio::test]
async fn common_deletion_marks_existing_common_message_but_not_same_id_in_channel() {
    let (_directory, store) = store().await;
    store
        .bind_telegram_account(SenderId::from_marked(1).unwrap())
        .await
        .unwrap();
    let common_chat = ChatId::from_marked(9).unwrap();
    let channel_chat = ChatId::from_marked(-1_000_000_000_009).unwrap();
    store
        .write_batch(insert_messages(vec![
            message(common_chat.get(), 18),
            message(channel_chat.get(), 18),
        ]))
        .await
        .unwrap();
    store
        .write_batch(IngestBatch {
            account_deletions: vec![AccountDeletion {
                message_id: MessageId::new(18).unwrap(),
                deleted_at: DateTime::from_timestamp(1_700_000_020, 0).unwrap(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(
        store
            .get(common_chat, MessageId::new(18).unwrap(), false)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get(channel_chat, MessageId::new(18).unwrap(), false)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(store.unresolved_common_deletion_count().await.unwrap(), 0);
}

#[tokio::test]
async fn ambiguous_common_id_is_tombstoned_but_never_guessed() {
    let (_directory, store) = store().await;
    store
        .bind_telegram_account(SenderId::from_marked(1).unwrap())
        .await
        .unwrap();
    let first_chat = ChatId::from_marked(201).unwrap();
    let second_chat = ChatId::from_marked(-202).unwrap();
    let id = MessageId::new(32).unwrap();
    store
        .write_batch(insert_messages(vec![
            message(first_chat.get(), 32),
            message(second_chat.get(), 32),
        ]))
        .await
        .unwrap();
    store
        .write_batch(IngestBatch {
            account_deletions: vec![AccountDeletion {
                message_id: id,
                deleted_at: DateTime::from_timestamp(1_700_000_020, 0).unwrap(),
            }],
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(store.get(first_chat, id, false).await.unwrap().is_some());
    assert!(store.get(second_chat, id, false).await.unwrap().is_some());
    assert_eq!(store.unresolved_common_deletion_count().await.unwrap(), 1);
    assert!(
        store
            .get(ChatId::from_marked(-1_000_000_000_201).unwrap(), id, false)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn archive_account_binding_is_stable_and_rejects_a_different_user() {
    let (_directory, store) = store().await;
    let first = SenderId::from_marked(101).unwrap();
    store.bind_telegram_account(first).await.unwrap();
    store.bind_telegram_account(first).await.unwrap();
    let error = store
        .bind_telegram_account(SenderId::from_marked(102).unwrap())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("bound to Telegram user 101"));
}

#[tokio::test]
async fn account_wide_deletion_requires_archive_identity() {
    let (_directory, store) = store().await;
    let result = store
        .write_batch(IngestBatch {
            account_deletions: vec![AccountDeletion {
                message_id: MessageId::new(5).unwrap(),
                deleted_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            }],
            ..Default::default()
        })
        .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("bind the Telegram account")
    );
}

#[tokio::test]
async fn replaying_the_same_realtime_batch_after_a_crash_leaves_one_row() {
    let (_directory, store) = store().await;
    let chat_id = ChatId::from_marked(201).unwrap();
    let batch = insert_messages(vec![message(chat_id.get(), 41)]);
    store.write_batch(batch.clone()).await.unwrap();
    store.write_batch(batch).await.unwrap();

    let page = store
        .list(tgarchive::application::ListMessagesQuery {
            filters: tgarchive::application::MessageFilters {
                chat_id: Some(chat_id),
                sender_id: None,
                post_author: None,
                time_range: tgarchive::application::TimeRange::new(None, None).unwrap(),
                include_deleted: false,
            },
            before: None,
            after: None,
            page_size: tgarchive::application::PageSize::DEFAULT,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
}

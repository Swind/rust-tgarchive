use chrono::{DateTime, Utc};
use tgarchive::{
    application::{ArchiveWriter, ChatRepository, IngestBatch, IngestRecord, MessageSource},
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId,
    },
    infrastructure::persistence::sqlite::{SqliteStore, media::MediaPolicy},
};

async fn setup() -> (tempfile::TempDir, SqliteStore, ChatId) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("media.db").display());
    let store = SqliteStore::connect(&url).await.unwrap();
    let id = ChatId::from_marked(24).unwrap();
    store
        .save_refresh(vec![Chat {
            id,
            kind: ChatKind::Group,
            title: None,
            username: None,
            tracked: false,
        }])
        .await
        .unwrap();
    (dir, store, id)
}

fn message(chat_id: ChatId, id: i64, attachments: Vec<Attachment>) -> Message {
    let now = DateTime::<Utc>::from_timestamp(1_700_000_000 + id, 0).unwrap();
    Message {
        post_author: None,
        forward: None,
        id: MessageId::new(id).unwrap(),
        chat_id,
        sender_id: None,
        timestamp: now,
        edited_at: None,
        collected_at: now,
        text: None,
        reply_to: None,
        attachments,
    }
}

fn image(kind: AttachmentKind, file_id: &str, mime: &str) -> Attachment {
    Attachment {
        kind,
        telegram_file_id: Some(file_id.into()),
        mime_type: Some(mime.into()),
        file_name: None,
        size: None,
    }
}

async fn ingest(store: &SqliteStore, chat_id: ChatId, messages: Vec<Message>) {
    store
        .write_batch(IngestBatch {
            chats: vec![Chat {
                id: chat_id,
                kind: ChatKind::Group,
                title: None,
                username: None,
                tracked: false,
            }],
            records: messages
                .into_iter()
                .map(|m| IngestRecord {
                    event: MessageEvent::Created(m),
                    source: MessageSource::Realtime,
                })
                .collect(),
            ..Default::default()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn tracked_ingest_queues_supported_previews_and_policy_archive_idempotently() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    store
        .set_media_policy(chat.get(), MediaPolicy { auto_archive: true })
        .await
        .unwrap();
    let attachments = vec![
        image(AttachmentKind::Photo, "photo-id", "image/jpeg"),
        image(AttachmentKind::Document, "png-id", "image/png"),
        image(AttachmentKind::Document, "gif-id", "image/gif"),
        image(AttachmentKind::Video, "video-id", "video/mp4"),
    ];
    ingest(&store, chat, vec![message(chat, 1, attachments.clone())]).await;
    ingest(&store, chat, vec![message(chat, 1, attachments)]).await;
    let rows = store.message_media(chat.get(), 1).await.unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows.iter().filter(|m| m.variant == "preview").count(), 2);
    assert_eq!(rows.iter().filter(|m| m.variant == "archive").count(), 2);
    assert!(rows.iter().all(|m| m.state == "queued"));
}

#[tokio::test]
async fn manual_archive_deduplicates_and_disable_only_blocks_automatic_archive() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "p", "image/jpeg")],
        )],
    )
    .await;
    let preview = store.message_media(chat.get(), 1).await.unwrap().remove(0);
    store
        .set_media_policy(chat.get(), MediaPolicy { auto_archive: true })
        .await
        .unwrap();
    let archive = store.request_archive(preview.id).await.unwrap().unwrap();
    let again = store.request_archive(preview.id).await.unwrap().unwrap();
    assert_eq!(archive.id, again.id);
    assert_eq!(again.trigger, "manual");
    store
        .set_media_policy(
            chat.get(),
            MediaPolicy {
                auto_archive: false,
            },
        )
        .await
        .unwrap();
    assert!(store.media_allowed(&again).await.unwrap());
    assert!(store.media_allowed(&preview).await.unwrap());
    ChatRepository::set_tracked(&store, chat, false)
        .await
        .unwrap();
    assert!(store.media_allowed(&again).await.unwrap());
    assert!(!store.media_allowed(&preview).await.unwrap());
}

#[tokio::test]
async fn disabled_automatic_archive_can_be_requested_manually_again() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    store
        .set_media_policy(chat.get(), MediaPolicy { auto_archive: true })
        .await
        .unwrap();
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "p", "image/jpeg")],
        )],
    )
    .await;
    let jobs = store.message_media(chat.get(), 1).await.unwrap();
    let preview = jobs.iter().find(|job| job.variant == "preview").unwrap();
    let archive = jobs.iter().find(|job| job.variant == "archive").unwrap();
    store
        .set_media_policy(
            chat.get(),
            MediaPolicy {
                auto_archive: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_media(archive.id).await.unwrap().unwrap().state,
        "interrupted"
    );
    let requested = store.request_archive(preview.id).await.unwrap().unwrap();
    assert_eq!(requested.id, archive.id);
    assert_eq!(requested.state, "queued");
    assert_eq!(requested.trigger, "manual");
    assert_eq!(requested.attempts, 0);
    assert!(store.media_allowed(&requested).await.unwrap());
}

#[tokio::test]
async fn untracked_and_deleted_messages_are_not_current_or_allowed() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "p", "image/jpeg")],
        )],
    )
    .await;
    let preview = store.message_media(chat.get(), 1).await.unwrap().remove(0);
    ChatRepository::set_tracked(&store, chat, false)
        .await
        .unwrap();
    assert!(!store.media_allowed(&preview).await.unwrap());
    assert!(store.media_is_current(&preview).await.unwrap());
    let deleted_at = DateTime::<Utc>::from_timestamp(1_700_000_100, 0).unwrap();
    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Deleted {
                    chat_id: chat,
                    message_id: MessageId::new(1).unwrap(),
                    deleted_at,
                },
                source: MessageSource::Realtime,
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!store.media_is_current(&preview).await.unwrap());
    assert!(store.message_media(chat.get(), 1).await.unwrap().is_empty());
}

#[tokio::test]
async fn replaced_attachment_keeps_old_file_identity_and_rejects_stale_archive_request() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "old-id", "image/jpeg")],
        )],
    )
    .await;
    let old = store.message_media(chat.get(), 1).await.unwrap().remove(0);
    let old_archive = store.request_archive(old.id).await.unwrap().unwrap();
    let mut edited = message(
        chat,
        1,
        vec![image(AttachmentKind::Photo, "new-id", "image/jpeg")],
    );
    edited.edited_at = Some(DateTime::<Utc>::from_timestamp(1_700_000_010, 0).unwrap());
    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Updated(edited),
                source: MessageSource::Realtime,
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!store.media_is_current(&old).await.unwrap());
    assert!(store.request_archive(old.id).await.unwrap().is_none());
    let current = store.message_media(chat.get(), 1).await.unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].telegram_media_id, "new-id");
    assert!(!store.media_is_current(&old_archive).await.unwrap());
}

#[tokio::test]
async fn manual_backfill_works_with_auto_archive_off_and_null_retry_is_not_claimed() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "p", "image/jpeg")],
        )],
    )
    .await;
    let rows = store.enqueue_media(chat.get(), "archive").await.unwrap();
    let archive = rows.iter().find(|m| m.variant == "archive").unwrap();
    assert_eq!(archive.trigger, "manual");
    let claimed = store.claim_media().await.unwrap().unwrap();
    store
        .fail_media(claimed.id, "failed", "temporary", None)
        .await
        .unwrap();
    let claimed = store.claim_media().await.unwrap().unwrap();
    store
        .fail_media(claimed.id, "failed", "temporary", None)
        .await
        .unwrap();
    assert!(store.claim_media().await.unwrap().is_none());
}

#[tokio::test]
async fn finish_rechecks_tracking_and_current_identity() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "p", "image/jpeg")],
        )],
    )
    .await;
    let job = store.claim_media().await.unwrap().unwrap();
    ChatRepository::set_tracked(&store, chat, false)
        .await
        .unwrap();
    assert!(
        store
            .finish_media(job.id, "media/1.webp", "image/webp", 10, None, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn refetch_schedules_the_attachment_identity_already_stored() {
    let (_dir, store, chat) = setup().await;
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "stored-id", "image/jpeg")],
        )],
    )
    .await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Updated(message(
                    chat,
                    1,
                    vec![image(AttachmentKind::Photo, "incoming-id", "image/jpeg")],
                )),
                source: MessageSource::Refetch,
            }],
            ..Default::default()
        })
        .await
        .unwrap();
    let rows = store.message_media(chat.get(), 1).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].telegram_media_id, "stored-id");
}

#[tokio::test]
async fn missing_completed_media_can_be_requeued_without_resurrecting_stale_identity() {
    let (_dir, store, chat) = setup().await;
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    ingest(
        &store,
        chat,
        vec![message(
            chat,
            1,
            vec![image(AttachmentKind::Photo, "p", "image/jpeg")],
        )],
    )
    .await;
    let job = store.claim_media().await.unwrap().unwrap();
    store
        .finish_media(
            job.id,
            "1.preview.jpg",
            "image/jpeg",
            10,
            Some(10),
            Some(10),
        )
        .await
        .unwrap();
    assert_eq!(store.list_completed_media().await.unwrap().len(), 1);
    assert!(store.invalidate_missing_media(job.id).await.unwrap());
    assert!(!store.invalidate_missing_media(job.id).await.unwrap());
    assert_eq!(
        store.get_media(job.id).await.unwrap().unwrap().state,
        "queued"
    );
}

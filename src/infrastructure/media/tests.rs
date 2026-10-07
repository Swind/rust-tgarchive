use super::*;
use crate::{
    application::{ArchiveWriter, ChatRepository, IngestBatch, IngestRecord, MessageSource},
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId,
    },
};
use chrono::{DateTime, Utc};

async fn fixture() -> (tempfile::TempDir, SqliteStore, ChatId) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::connect(&format!(
        "sqlite://{}",
        dir.path().join("worker.db").display()
    ))
    .await
    .unwrap();
    let chat = ChatId::from_marked(24).unwrap();
    store
        .save_refresh(vec![Chat {
            id: chat,
            kind: ChatKind::Group,
            title: None,
            username: None,
            tracked: false,
        }])
        .await
        .unwrap();
    ChatRepository::set_tracked(&store, chat, true)
        .await
        .unwrap();
    (dir, store, chat)
}

async fn ingest_images(store: &SqliteStore, chat: ChatId, count: i64) {
    let records = (1..=count)
        .map(|id| {
            let timestamp = DateTime::<Utc>::from_timestamp(1_700_000_000 + id, 0).unwrap();
            IngestRecord {
                event: MessageEvent::Created(Message {
                    post_author: None,
                    forward: None,
                    id: MessageId::new(id).unwrap(),
                    chat_id: chat,
                    sender_id: None,
                    timestamp,
                    edited_at: None,
                    collected_at: timestamp,
                    text: None,
                    reply_to: None,
                    attachments: vec![Attachment {
                        kind: AttachmentKind::Photo,
                        telegram_file_id: Some(format!("photo-{id}")),
                        mime_type: Some("image/jpeg".into()),
                        file_name: None,
                        size: None,
                    }],
                }),
                source: MessageSource::Realtime,
            }
        })
        .collect();
    store
        .write_batch(IngestBatch {
            records,
            ..Default::default()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn image_type_reads_real_files_and_rejects_unknown_content() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("image");
    tokio::fs::write(&png, b"\x89PNG\r\n\x1a\nrest")
        .await
        .unwrap();
    assert_eq!(image_type(&png).await.unwrap(), ("image/png", "png"));
    let bad = dir.path().join("bad");
    tokio::fs::write(&bad, b"not an image").await.unwrap();
    assert!(matches!(
        image_type(&bad).await,
        Err(WorkerError::InvalidImage)
    ));
    assert!(checked_media_size(0, MAX_BYTES).is_some());
    assert!(checked_media_size(MAX_BYTES, 1).is_none());
}

#[tokio::test]
async fn startup_removes_only_partial_files() {
    let dir = tempfile::tempdir().unwrap();
    let part = dir.path().join("7.preview.part");
    let final_file = dir.path().join("7.preview.jpg");
    tokio::fs::write(&part, b"partial").await.unwrap();
    tokio::fs::write(&final_file, b"final").await.unwrap();
    cleanup_partials(dir.path()).await.unwrap();
    assert!(!part.exists());
    assert!(final_file.exists());
}

#[tokio::test]
async fn startup_reconciliation_preserves_valid_files_and_requeues_missing_or_corrupt() {
    let (dir, store, chat) = fixture().await;
    let media_dir = dir.path().join("media");
    tokio::fs::create_dir_all(&media_dir).await.unwrap();
    ingest_images(&store, chat, 3).await;
    let mut jobs = Vec::new();
    for id in 1..=3 {
        jobs.push(store.message_media(chat.get(), id).await.unwrap().remove(0));
    }
    for (index, job) in jobs.iter().enumerate() {
        let claimed = store.claim_media().await.unwrap().unwrap();
        assert_eq!(claimed.id, job.id);
        let name = format!("{}.{}.png", job.id, job.variant);
        let bytes: &[u8] = if index == 1 {
            b"broken data"
        } else {
            b"\x89PNG\r\n\x1a\nvalid"
        };
        if index != 2 {
            tokio::fs::write(media_dir.join(&name), bytes)
                .await
                .unwrap();
        }
        store
            .finish_media(job.id, &name, "image/png", bytes.len() as i64, None, None)
            .await
            .unwrap();
    }
    reconcile_completed(&store, &media_dir).await.unwrap();
    assert_eq!(
        store.get_media(jobs[0].id).await.unwrap().unwrap().state,
        "succeeded"
    );
    for job in &jobs[1..] {
        let row = store.get_media(job.id).await.unwrap().unwrap();
        assert_eq!(row.state, "queued");
        assert_eq!(row.relative_path, None);
    }
}

#[tokio::test]
async fn stopped_jobs_recover_and_flood_waits_are_durable_and_bounded() {
    let (_dir, store, chat) = fixture().await;
    ingest_images(&store, chat, 1).await;
    let job = store.claim_media().await.unwrap().unwrap();
    let cancel = CancellationToken::new();
    let pacer = RatePacer::disabled();
    report_failure(&store, &pacer, &cancel, &job, WorkerError::Stopped).await;
    assert_eq!(
        store.get_media(job.id).await.unwrap().unwrap().state,
        "interrupted"
    );
    store.recover_media().await.unwrap();
    assert_eq!(
        store.get_media(job.id).await.unwrap().unwrap().state,
        "queued"
    );

    let claimed = store.claim_media().await.unwrap().unwrap();
    let now = chrono::Utc::now().timestamp();
    let flood = WorkerError::Telegram(grammers_client::InvocationError::Rpc(
        grammers_client::sender::RpcError {
            code: 420,
            name: "FLOOD_WAIT".into(),
            value: Some(30),
            caused_by: None,
        },
    ));
    report_failure(&store, &pacer, &cancel, &claimed, flood).await;
    let row = store.get_media(job.id).await.unwrap().unwrap();
    assert_eq!(row.state, "failed");
    assert!(row.next_attempt_at.unwrap() >= now + 29);
    assert_eq!(pacer.status().last_flood_wait_secs, Some(30));

    let mut exhausted = claimed;
    exhausted.attempts = MAX_ATTEMPTS;
    let flood = WorkerError::Telegram(grammers_client::InvocationError::Rpc(
        grammers_client::sender::RpcError {
            code: 420,
            name: "FLOOD_WAIT".into(),
            value: Some(30),
            caused_by: None,
        },
    ));
    report_failure(&store, &pacer, &cancel, &exhausted, flood).await;
    assert_eq!(
        store.get_media(job.id).await.unwrap().unwrap().state,
        "interrupted"
    );
}

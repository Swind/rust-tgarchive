use chrono::{DateTime, Utc};
use tgarchive::{
    application::{
        ArchiveWriter, ChatCheckpoint, ChatRepository, IngestBatch, IngestRecord,
        ListMessagesQuery, MessageCursor, MessageFilters, MessageRepository, MessageSource,
        PageSize, SearchMessagesQuery, SyncChatProgress, SyncJob, SyncJobState, SyncRepository,
        SyncScope, TimeRange,
    },
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId,
    },
    infrastructure::persistence::sqlite::SqliteStore,
};

async fn store() -> (tempfile::TempDir, SqliteStore, String) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = SqliteStore::connect(&url).await.unwrap();
    (dir, store, url)
}

fn chat(id: i64) -> Chat {
    Chat {
        id: ChatId::from_marked(id).unwrap(),
        kind: if id > 0 {
            ChatKind::Private
        } else {
            ChatKind::Group
        },
        title: Some(format!("chat {id}")),
        username: None,
        tracked: false,
    }
}

fn message(chat_id: ChatId, id: i64, text: &str) -> Message {
    let time = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    Message {
        id: MessageId::new(id).unwrap(),
        chat_id,
        sender_id: None,
        timestamp: time,
        edited_at: None,
        collected_at: time,
        text: Some(text.to_owned()),
        reply_to: None,
        attachments: Vec::new(),
    }
}

fn ingest(messages: Vec<Message>) -> IngestBatch {
    let chats = messages
        .iter()
        .map(|message| chat(message.chat_id.get()))
        .collect();
    IngestBatch {
        chats,
        records: messages
            .into_iter()
            .map(|message| IngestRecord {
                event: MessageEvent::Created(message),
                source: MessageSource::History,
            })
            .collect(),
        ..IngestBatch::default()
    }
}

fn filters() -> MessageFilters {
    MessageFilters {
        chat_id: None,
        sender_id: None,
        time_range: TimeRange::new(None, None).unwrap(),
        include_deleted: false,
    }
}

fn list(
    before: Option<MessageCursor>,
    after: Option<MessageCursor>,
    limit: u16,
) -> ListMessagesQuery {
    ListMessagesQuery {
        filters: filters(),
        before,
        after,
        page_size: PageSize::new(limit).unwrap(),
    }
}

#[tokio::test]
async fn file_database_reopens_readonly_and_preserves_ft_search() {
    let (_dir, store, url) = store().await;
    let chat_id = ChatId::from_marked(10).unwrap();
    store
        .write_batch(ingest(vec![
            message(chat_id, 1, "alpha OR beta and quote \" safely"),
            message(chat_id, 2, "中文測試"),
        ]))
        .await
        .unwrap();

    let search = SearchMessagesQuery {
        text: "alpha OR beta".into(),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    let found = store.search(search).await.unwrap();
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].id.get(), 1);

    let chinese = SearchMessagesQuery {
        text: "中文測試".into(),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    assert_eq!(store.search(chinese).await.unwrap().items.len(), 1);
    let quoted = SearchMessagesQuery {
        text: "alpha\" OR *".into(),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    assert!(store.search(quoted).await.is_ok());
    store.close().await;

    let reopened_writer = SqliteStore::connect(&url).await.unwrap();
    assert_eq!(
        ChatRepository::list(&reopened_writer).await.unwrap().len(),
        1
    );
    reopened_writer.close().await;

    let reopened = SqliteStore::open_existing_readonly(&url).await.unwrap();
    assert!(
        MessageRepository::get(&reopened, chat_id, MessageId::new(2).unwrap(), false)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(ChatRepository::list(&reopened).await.unwrap().len(), 1);
    reopened.close().await;
}

#[tokio::test]
async fn concurrent_wal_reads_and_writes_keep_committed_batches_visible() {
    let (_dir, store, _) = store().await;
    let read = async { ChatRepository::list(&store).await.unwrap().len() };
    let write_a = async {
        store
            .write_batch(ingest(vec![message(
                ChatId::from_marked(81).unwrap(),
                1,
                "a",
            )]))
            .await
            .unwrap();
    };
    let write_b = async {
        store
            .write_batch(ingest(vec![message(
                ChatId::from_marked(82).unwrap(),
                1,
                "b",
            )]))
            .await
            .unwrap();
    };
    let (before_or_during, (), ()) = tokio::join!(read, write_a, write_b);
    assert!(before_or_during <= 2);
    assert_eq!(ChatRepository::list(&store).await.unwrap().len(), 2);
}

#[tokio::test]
async fn keyset_pages_order_same_second_across_chat_ids_in_both_directions() {
    let (_dir, store, _) = store().await;
    let ids = [1, -2, -3].map(|id| ChatId::from_marked(id).unwrap());
    store
        .write_batch(ingest(vec![
            message(ids[0], 1, "a"),
            message(ids[1], 1, "b"),
            message(ids[2], 1, "c"),
        ]))
        .await
        .unwrap();

    let first = MessageRepository::list(&store, list(None, None, 2))
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|m| m.chat_id.get())
            .collect::<Vec<_>>(),
        vec![1, -2]
    );
    assert!(first.has_more);
    let next = MessageRepository::list(&store, list(first.next_cursor, None, 2))
        .await
        .unwrap();
    assert_eq!(
        next.items
            .iter()
            .map(|m| m.chat_id.get())
            .collect::<Vec<_>>(),
        vec![-3]
    );

    let newer = MessageRepository::list(
        &store,
        list(
            None,
            Some(MessageCursor {
                timestamp: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
                chat_id: ids[2],
                message_id: MessageId::new(1).unwrap(),
            }),
            2,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        newer
            .items
            .iter()
            .map(|m| m.chat_id.get())
            .collect::<Vec<_>>(),
        vec![1, -2]
    );
}

#[tokio::test]
async fn failed_batch_rolls_back_message_attachments_and_checkpoint() {
    let (_dir, store, _) = store().await;
    let chat_id = ChatId::from_marked(22).unwrap();
    let mut broken = message(chat_id, 8, "must roll back");
    broken.attachments.push(Attachment {
        kind: AttachmentKind::Document,
        telegram_file_id: None,
        mime_type: None,
        file_name: None,
        size: Some(-1),
    });
    let mut batch = ingest(vec![broken]);
    batch.checkpoint = Some((
        chat_id,
        ChatCheckpoint {
            history_before_id: Some(MessageId::new(7).unwrap()),
            history_complete: false,
            catchup_after_id: None,
        },
    ));
    assert!(store.write_batch(batch).await.is_err());
    assert!(
        MessageRepository::get(&store, chat_id, MessageId::new(8).unwrap(), false)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.get_checkpoint(chat_id).await.unwrap().is_none());
}

#[tokio::test]
async fn tombstone_prevents_later_history_from_resurrecting_message() {
    let (_dir, store, _) = store().await;
    let chat_id = ChatId::from_marked(31).unwrap();
    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Deleted {
                    chat_id,
                    message_id: MessageId::new(9).unwrap(),
                    deleted_at: DateTime::from_timestamp(1_700_000_002, 0).unwrap(),
                },
                source: MessageSource::Realtime,
            }],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    store
        .write_batch(ingest(vec![message(chat_id, 9, "deleted")]))
        .await
        .unwrap();
    assert!(
        MessageRepository::get(&store, chat_id, MessageId::new(9).unwrap(), false)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(ChatRepository::list(&store).await.unwrap().len(), 1);
}

#[tokio::test]
async fn versions_prefer_edits_and_realtime_and_soft_delete_hides_fts() {
    let (_dir, store, _) = store().await;
    let chat_id = ChatId::from_marked(50).unwrap();
    let mut latest = message(chat_id, 7, "needle edited");
    latest.edited_at = Some(DateTime::from_timestamp(1_700_000_010, 0).unwrap());
    latest.attachments.push(Attachment {
        kind: AttachmentKind::Photo,
        telegram_file_id: Some("new-file".into()),
        mime_type: None,
        file_name: None,
        size: None,
    });
    store
        .write_batch(IngestBatch {
            chats: vec![chat(50)],
            records: vec![
                IngestRecord {
                    event: MessageEvent::Created(latest.clone()),
                    source: MessageSource::Realtime,
                },
                IngestRecord {
                    event: MessageEvent::Updated(latest.clone()),
                    source: MessageSource::Realtime,
                },
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();

    store
        .write_batch(ingest(vec![message(chat_id, 7, "needle stale")]))
        .await
        .unwrap();
    let saved = MessageRepository::get(&store, chat_id, MessageId::new(7).unwrap(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.text.as_deref(), Some("needle edited"));
    assert_eq!(
        saved.attachments[0].telegram_file_id.as_deref(),
        Some("new-file")
    );

    let mut same_version_history = message(chat_id, 7, "needle history");
    same_version_history.edited_at = latest.edited_at;
    store
        .write_batch(ingest(vec![same_version_history]))
        .await
        .unwrap();
    let saved = MessageRepository::get(&store, chat_id, MessageId::new(7).unwrap(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.text.as_deref(), Some("needle edited"));
    assert_eq!(saved.attachments.len(), 1);

    let mut newer_edit = message(chat_id, 7, "fresh text");
    newer_edit.edited_at = Some(DateTime::from_timestamp(1_700_000_020, 0).unwrap());
    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Updated(newer_edit),
                source: MessageSource::Realtime,
            }],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let old_term = SearchMessagesQuery {
        text: "edited".into(),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    assert!(store.search(old_term).await.unwrap().items.is_empty());
    let new_term = SearchMessagesQuery {
        text: "fresh".into(),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    assert_eq!(store.search(new_term).await.unwrap().items.len(), 1);

    store
        .write_batch(IngestBatch {
            records: vec![IngestRecord {
                event: MessageEvent::Deleted {
                    chat_id,
                    message_id: MessageId::new(7).unwrap(),
                    deleted_at: DateTime::from_timestamp(1_700_000_020, 0).unwrap(),
                },
                source: MessageSource::Realtime,
            }],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let search = SearchMessagesQuery {
        text: "needle".into(),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    assert!(store.search(search).await.unwrap().items.is_empty());
}

#[tokio::test]
async fn fractional_time_filters_and_cursors_match_second_precision_storage() {
    let (_dir, store, _) = store().await;
    let chat_id = ChatId::from_marked(60).unwrap();
    let first = message(chat_id, 1, "first");
    let mut second = message(chat_id, 2, "second");
    second.timestamp = DateTime::from_timestamp(1_700_000_001, 0).unwrap();
    store
        .write_batch(ingest(vec![first, second]))
        .await
        .unwrap();

    let half = DateTime::from_timestamp(1_700_000_000, 500_000_000).unwrap();
    let from_half = ListMessagesQuery {
        filters: MessageFilters {
            chat_id: None,
            sender_id: None,
            time_range: TimeRange::new(Some(half), None).unwrap(),
            include_deleted: false,
        },
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    assert_eq!(
        MessageRepository::list(&store, from_half)
            .await
            .unwrap()
            .items
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![2]
    );
    let to_half = ListMessagesQuery {
        filters: MessageFilters {
            chat_id: None,
            sender_id: None,
            time_range: TimeRange::new(None, Some(half)).unwrap(),
            include_deleted: false,
        },
        before: None,
        after: None,
        page_size: PageSize::new(10).unwrap(),
    };
    assert_eq!(
        MessageRepository::list(&store, to_half)
            .await
            .unwrap()
            .items
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1]
    );

    let cursor = MessageCursor {
        timestamp: half,
        chat_id,
        message_id: MessageId::new(1).unwrap(),
    };
    assert_eq!(
        MessageRepository::list(&store, list(Some(cursor.clone()), None, 10))
            .await
            .unwrap()
            .items
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(
        MessageRepository::list(&store, list(None, Some(cursor), 10))
            .await
            .unwrap()
            .items
            .iter()
            .map(|message| message.id.get())
            .collect::<Vec<_>>(),
        vec![2]
    );
}

#[tokio::test]
async fn jobs_transition_and_recovery_persist_chat_progress() {
    let (_dir, store, _) = store().await;
    let chat_id = ChatId::from_marked(70).unwrap();
    ChatRepository::save_refresh(&store, vec![chat(70)])
        .await
        .unwrap();
    let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let queued = SyncJob {
        id: "job-1".into(),
        scope: SyncScope::Chat(chat_id),
        state: SyncJobState::Queued,
        created_at: now,
        started_at: None,
        completed_at: None,
        summary_error: None,
    };
    SyncRepository::save_job(&store, queued.clone())
        .await
        .unwrap();
    let mut running = queued.clone();
    running.state = SyncJobState::Running;
    running.started_at = Some(now);
    SyncRepository::save_job(&store, running.clone())
        .await
        .unwrap();
    let mut changed_scope = running.clone();
    changed_scope.scope = SyncScope::All;
    assert!(
        SyncRepository::save_job(&store, changed_scope)
            .await
            .is_err()
    );
    store
        .write_batch(IngestBatch {
            job_progress: Some(SyncChatProgress {
                job_id: "job-1".into(),
                chat_id,
                state: SyncJobState::Running,
                committed_messages: 4,
                summary_error: None,
            }),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    SyncRepository::recover_interrupted(&store).await.unwrap();
    let recovered = SyncRepository::get_job(&store, "job-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.state, SyncJobState::Interrupted);
    assert_eq!(
        SyncRepository::list_chat_progress(&store, "job-1")
            .await
            .unwrap()[0]
            .state,
        SyncJobState::Interrupted
    );
    assert!(SyncRepository::save_job(&store, running).await.is_err());
}

#[tokio::test]
async fn history_and_catchup_checkpoints_merge_monotonically_in_one_writer() {
    let (_dir, store, _) = store().await;
    let chat_id = ChatId::from_marked(71).unwrap();
    for checkpoint in [
        ChatCheckpoint {
            history_before_id: Some(MessageId::new(80).unwrap()),
            history_complete: false,
            catchup_after_id: Some(MessageId::new(20).unwrap()),
        },
        ChatCheckpoint {
            history_before_id: Some(MessageId::new(60).unwrap()),
            history_complete: false,
            catchup_after_id: Some(MessageId::new(30).unwrap()),
        },
        ChatCheckpoint {
            history_before_id: Some(MessageId::new(70).unwrap()),
            history_complete: true,
            catchup_after_id: Some(MessageId::new(25).unwrap()),
        },
    ] {
        store
            .write_batch(IngestBatch {
                checkpoint: Some((chat_id, checkpoint)),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let saved = SyncRepository::get_checkpoint(&store, chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.history_before_id.unwrap().get(), 60);
    assert!(saved.history_complete);
    assert_eq!(saved.catchup_after_id.unwrap().get(), 30);
}

#[tokio::test]
async fn empty_chat_baseline_roundtrips_and_last_error_is_set_then_cleared() {
    let (_dir, store, url) = store().await;
    let chat_id = ChatId::from_marked(72).unwrap();
    store
        .write_batch(IngestBatch {
            checkpoint: Some((
                chat_id,
                ChatCheckpoint {
                    history_before_id: None,
                    history_complete: false,
                    catchup_after_id: Some(MessageId::BEFORE_FIRST),
                },
            )),
            ..Default::default()
        })
        .await
        .unwrap();
    let saved = SyncRepository::get_checkpoint(&store, chat_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.catchup_after_id, Some(MessageId::BEFORE_FIRST));

    store
        .write_batch(IngestBatch {
            chat_error: Some((chat_id, "peer unavailable".into())),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        last_error(&url, chat_id).await.as_deref(),
        Some("peer unavailable")
    );

    store
        .write_batch(IngestBatch {
            checkpoint: Some((chat_id, saved)),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(last_error(&url, chat_id).await, None);
}

async fn last_error(url: &str, chat_id: ChatId) -> Option<String> {
    use sqlx::{Connection, Row};
    let mut connection = sqlx::SqliteConnection::connect(url).await.unwrap();
    sqlx::query("SELECT last_error FROM chat_sync_state WHERE chat_id=?")
        .bind(chat_id.get())
        .fetch_one(&mut connection)
        .await
        .unwrap()
        .get("last_error")
}

#[tokio::test]
async fn successful_chat_progress_records_sync_start_and_completion_times() {
    let (_dir, store, _) = store().await;
    let chat_id = ChatId::from_marked(81).unwrap();
    store
        .write_batch(ingest(vec![message(chat_id, 1, "x")]))
        .await
        .unwrap();
    let now = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    SyncRepository::save_job(
        &store,
        SyncJob {
            id: "j".into(),
            scope: SyncScope::Chat(chat_id),
            state: SyncJobState::Running,
            created_at: now,
            started_at: Some(now),
            completed_at: None,
            summary_error: None,
        },
    )
    .await
    .unwrap();
    let progress = |state| IngestBatch {
        job_progress: Some(SyncChatProgress {
            job_id: "j".into(),
            chat_id,
            state,
            committed_messages: 1,
            summary_error: None,
        }),
        ..IngestBatch::default()
    };
    let times = || async {
        let stats = ChatRepository::get_with_stats(&store, chat_id)
            .await
            .unwrap()
            .unwrap()
            .stats;
        stats.last_sync_completed_at
    };
    store
        .write_batch(progress(SyncJobState::Running))
        .await
        .unwrap();
    assert!(times().await.is_none());
    store
        .write_batch(progress(SyncJobState::Succeeded))
        .await
        .unwrap();
    assert!(times().await.is_some());
}

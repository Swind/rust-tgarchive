//! Sender attribution: sender endpoints, post author / forward metadata, `repair senders`,
//! `--refetch` backfill, migration 0007 and CLI sender resolution.

use std::{
    collections::VecDeque,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use chrono::DateTime;
use serde_json::Value;
use tgarchive::{
    application::{
        ArchiveWriter, HistoryBoundary, HistoryPage, IngestBatch, IngestRecord, MessageRepository,
        MessageSource, PageSize, SyncJob, SyncJobState, SyncRepository, SyncScope, TelegramError,
        TelegramGateway, ingestion_worker,
        services::{Application, ComponentState, ComponentStatus},
        sync::{CancellationToken, SyncEngine},
    },
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Forward, Message, MessageEvent,
        MessageId, Sender, SenderId, SenderKind,
    },
    infrastructure::persistence::sqlite::SqliteStore,
    interface::rest::router,
};
use tower::ServiceExt;

const BIN: &str = env!("CARGO_BIN_EXE_tgarchive");
const BASE: i64 = 1_700_000_000;

fn group(raw: i64) -> ChatId {
    ChatId::from_telegram(ChatKind::Group, raw).unwrap()
}

fn channel(raw: i64) -> ChatId {
    ChatId::from_telegram(ChatKind::Channel, raw).unwrap()
}

fn user(raw: i64) -> SenderId {
    SenderId::from_telegram(SenderKind::User, raw).unwrap()
}

fn chat(id: ChatId, kind: ChatKind, title: &str) -> Chat {
    Chat {
        id,
        kind,
        title: Some(title.into()),
        username: None,
        tracked: true,
    }
}

fn person(raw: i64, name: &str, username: Option<&str>) -> Sender {
    Sender {
        id: user(raw),
        kind: SenderKind::User,
        display_name: Some(name.into()),
        username: username.map(str::to_owned),
    }
}

fn msg(chat_id: ChatId, id: i64, at: i64, sender: Option<SenderId>) -> Message {
    let time = DateTime::from_timestamp(BASE + at, 0).unwrap();
    Message {
        id: MessageId::new(id).unwrap(),
        chat_id,
        sender_id: sender,
        timestamp: time,
        edited_at: None,
        collected_at: time,
        text: Some(format!("body {id}")),
        reply_to: None,
        attachments: vec![],
        post_author: None,
        forward: None,
    }
}

fn record(message: Message, source: MessageSource) -> IngestRecord {
    IngestRecord {
        event: MessageEvent::Created(message),
        source,
    }
}

fn deleted(chat_id: ChatId, id: i64) -> IngestRecord {
    IngestRecord {
        event: MessageEvent::Deleted {
            chat_id,
            message_id: MessageId::new(id).unwrap(),
            deleted_at: DateTime::from_timestamp(BASE + 500, 0).unwrap(),
        },
        source: MessageSource::Realtime,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    url: String,
    store: Arc<SqliteStore>,
    app: axum::Router,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let application = Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        None,
        ComponentStatus::new(ComponentState::Running, None),
    ));
    Fixture {
        dir,
        url,
        store,
        app: router(application),
    }
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn sender_ids(page: &Value) -> Vec<i64> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect()
}

/// Senders: 1 王小明 (two chats), 2 Alice Chen, 3 Alice Wu, 9 self ("Me").
async fn seed_senders(f: &Fixture) {
    f.store.bind_telegram_account(user(9)).await.unwrap();
    let (g1, g2, private) = (
        group(10),
        group(20),
        ChatId::from_telegram(ChatKind::Private, 5).unwrap(),
    );
    f.store
        .write_batch(IngestBatch {
            chats: vec![
                chat(g1, ChatKind::Group, "Dev Group"),
                chat(g2, ChatKind::Group, "第二群"),
                chat(private, ChatKind::Private, "Alice"),
            ],
            senders: vec![
                person(1, "王小明", Some("xiaoming")),
                person(2, "Alice Chen", Some("alice")),
                person(3, "Alice Wu", None),
                person(9, "Me", Some("me_account")),
            ],
            records: vec![
                record(msg(g1, 1, 10, Some(user(1))), MessageSource::History),
                record(msg(g1, 2, 20, Some(user(1))), MessageSource::History),
                record(msg(g1, 3, 30, Some(user(1))), MessageSource::History),
                record(msg(g2, 1, 40, Some(user(1))), MessageSource::History),
                record(msg(g2, 2, 50, Some(user(1))), MessageSource::History),
                record(msg(g1, 4, 60, Some(user(2))), MessageSource::History),
                record(msg(g1, 5, 70, Some(user(3))), MessageSource::History),
                record(msg(g1, 6, 80, Some(user(3))), MessageSource::History),
                record(msg(private, 1, 90, Some(user(9))), MessageSource::History),
                record(msg(private, 2, 100, Some(user(9))), MessageSource::History),
                deleted(g1, 3),
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn sender_search_matches_cjk_substring_username_id_and_case() {
    let f = fixture().await;
    seed_senders(&f).await;
    let q = |text: &str| format!("/api/v1/senders?q={}", urlencode(text));

    let (status, page) = get(&f.app, &q("小明")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sender_ids(&page), [1]);
    assert_eq!(page["items"][0]["display_name"], "王小明");
    assert_eq!(page["items"][0]["username"], "xiaoming");
    assert_eq!(page["items"][0]["kind"], "user");
    assert_eq!(page["items"][0]["is_self"], false);

    let (_, page) = get(&f.app, &q("ALICE")).await;
    assert_eq!(
        sender_ids(&page),
        [3, 2],
        "both Alices, most messages first"
    );
    let (_, page) = get(&f.app, &q("@alice")).await;
    assert_eq!(sender_ids(&page), [2], "@username is an exact match");
    let (_, page) = get(&f.app, &q("@ali")).await;
    assert!(sender_ids(&page).is_empty());
    let (_, page) = get(&f.app, &q("3")).await;
    assert_eq!(sender_ids(&page), [3], "numeric text matches the ID");
    let (_, page) = get(&f.app, &q("nobody")).await;
    assert!(sender_ids(&page).is_empty());

    let (_, page) = get(&f.app, &q("me")).await;
    let me: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["is_self"] == true)
        .collect();
    assert_eq!(me.len(), 1);
    assert_eq!(me[0]["id"], 9);
    f.store.close().await;
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn sender_counts_exclude_deleted_unless_requested_and_sorting_paginates() {
    let f = fixture().await;
    seed_senders(&f).await;

    let (_, page) = get(&f.app, "/api/v1/senders").await;
    // messages desc: sender 1 has 4 (one of 5 deleted), 3 has 2, 9 has 2, 2 has 1.
    assert_eq!(sender_ids(&page), [1, 3, 9, 2]);
    let first = &page["items"][0];
    assert_eq!(first["message_count"], 4);
    assert_eq!(first["chat_count"], 2);
    assert_eq!(first["first_message_at"], rfc(BASE + 10));
    assert_eq!(first["last_message_at"], rfc(BASE + 50));

    let (_, page) = get(&f.app, "/api/v1/senders?include_deleted=true").await;
    assert_eq!(page["items"][0]["message_count"], 5);
    assert_eq!(page["items"][0]["last_message_at"], rfc(BASE + 50));

    let (_, page) = get(&f.app, "/api/v1/senders?sort=last_message").await;
    assert_eq!(sender_ids(&page), [9, 3, 2, 1]);
    let (_, page) = get(&f.app, "/api/v1/senders?sort=name").await;
    // "alice chen" < "alice wu" < "me" < "王小明"
    assert_eq!(sender_ids(&page), [2, 3, 9, 1]);

    let (_, one) = get(&f.app, "/api/v1/senders?sort=name&limit=3").await;
    assert_eq!(sender_ids(&one), [2, 3, 9]);
    let cursor = one["next_cursor"].as_str().unwrap();
    let (_, two) = get(
        &f.app,
        &format!("/api/v1/senders?sort=name&limit=3&cursor={cursor}"),
    )
    .await;
    assert_eq!(sender_ids(&two), [1]);
    assert!(two["next_cursor"].is_null());

    let (status, _) = get(&f.app, "/api/v1/senders?cursor=bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = get(&f.app, "/api/v1/senders?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    f.store.close().await;
}

fn rfc(seconds: i64) -> String {
    DateTime::from_timestamp(seconds, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[tokio::test]
async fn sender_detail_lists_chat_breakdown_and_unknown_is_404() {
    let f = fixture().await;
    seed_senders(&f).await;

    let (status, detail) = get(&f.app, "/api/v1/senders/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["message_count"], 4);
    let chats = detail["chats"].as_array().unwrap();
    assert_eq!(chats.len(), 2);
    // Equal counts: the more recently active chat comes first.
    assert_eq!(chats[0]["chat_id"], -20);
    assert_eq!(chats[0]["message_count"], 2);
    assert_eq!(chats[0]["title"], "第二群");
    assert_eq!(chats[0]["kind"], "group");
    assert_eq!(chats[0]["last_message_at"], rfc(BASE + 50));
    assert_eq!(chats[1]["chat_id"], -10);
    assert_eq!(chats[1]["last_message_at"], rfc(BASE + 20));

    let (_, with_deleted) = get(&f.app, "/api/v1/senders/1?include_deleted=true").await;
    assert_eq!(with_deleted["chats"][0]["chat_id"], -10);
    assert_eq!(with_deleted["chats"][0]["message_count"], 3);

    let (_, me) = get(&f.app, "/api/v1/senders/9").await;
    assert_eq!(me["is_self"], true);
    assert_eq!(me["chats"][0]["kind"], "private");

    let (status, _) = get(&f.app, "/api/v1/senders/777").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = get(&f.app, "/api/v1/senders/0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A sender with a stored message but no `senders` row is still listed and found.
    f.store
        .write_batch(IngestBatch {
            records: vec![record(
                msg(group(10), 50, 200, Some(user(55))),
                MessageSource::History,
            )],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let (status, detail) = get(&f.app, "/api/v1/senders/55").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["message_count"], 1);
    assert!(detail["display_name"].is_null());
    f.store.close().await;
}

#[tokio::test]
async fn post_author_and_forward_roundtrip_and_post_author_filter() {
    let f = fixture().await;
    let ch = channel(7);
    let mut post = msg(ch, 1, 10, Some(SenderId::from_marked(ch.get()).unwrap()));
    post.post_author = Some("Editor".into());
    let mut forwarded = msg(ch, 2, 20, Some(SenderId::from_marked(ch.get()).unwrap()));
    forwarded.forward = Some(Forward {
        from_id: Some(user(42)),
        from_name: Some("Origin".into()),
        date: DateTime::from_timestamp(BASE - 100, 0),
    });
    let plain = msg(ch, 3, 30, Some(SenderId::from_marked(ch.get()).unwrap()));
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(ch, ChatKind::Channel, "News")],
            records: [post, forwarded, plain]
                .into_iter()
                .map(|m| record(m, MessageSource::History))
                .collect(),
            ..IngestBatch::default()
        })
        .await
        .unwrap();

    let (_, item) = get(&f.app, "/api/v1/chats/-1000000000007/messages/1").await;
    assert_eq!(item["post_author"], "Editor");
    assert!(item["forward"].is_null());
    let (_, item) = get(&f.app, "/api/v1/chats/-1000000000007/messages/2").await;
    assert!(item["post_author"].is_null());
    assert_eq!(item["forward"]["from_id"], 42);
    assert_eq!(item["forward"]["from_name"], "Origin");
    assert_eq!(item["forward"]["date"], rfc(BASE - 100));
    assert_eq!(
        item["sender_id"],
        ch.get(),
        "forwarding never changes the sender"
    );
    assert_eq!(item["sender"]["display_name"], "News");

    let (_, page) = get(&f.app, "/api/v1/messages?post_author=Editor").await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], 1);
    let (_, page) = get(&f.app, "/api/v1/messages/search?q=body&post_author=Editor").await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let (_, page) = get(&f.app, "/api/v1/messages?post_author=Nobody").await;
    assert!(page["items"].as_array().unwrap().is_empty());
    f.store.close().await;
}

async fn null_senders(url: &str) -> Vec<(i64, i64)> {
    let pool = sqlx::SqlitePool::connect(url).await.unwrap();
    let rows = sqlx::query_as("SELECT chat_id, message_id FROM messages WHERE sender_id IS NULL ORDER BY chat_id, message_id")
        .fetch_all(&pool)
        .await
        .unwrap();
    pool.close().await;
    rows
}

#[tokio::test]
async fn repair_senders_fixes_channels_only_reports_private_and_is_idempotent() {
    let f = fixture().await;
    let (ch1, ch2, private, grp) = (
        channel(7),
        channel(8),
        ChatId::from_telegram(ChatKind::Private, 5).unwrap(),
        group(10),
    );
    f.store
        .write_batch(IngestBatch {
            chats: vec![
                chat(ch1, ChatKind::Channel, "News"),
                chat(ch2, ChatKind::Channel, "Other"),
                chat(private, ChatKind::Private, "Alice"),
                chat(grp, ChatKind::Group, "Group"),
            ],
            records: vec![
                record(msg(ch1, 1, 1, None), MessageSource::History),
                record(msg(ch1, 2, 2, None), MessageSource::History),
                record(msg(ch1, 3, 3, Some(user(1))), MessageSource::History),
                record(msg(ch2, 1, 4, None), MessageSource::History),
                record(msg(private, 1, 5, None), MessageSource::History),
                record(msg(grp, 1, 6, None), MessageSource::History),
                deleted(ch1, 2),
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();

    let dry = f.store.repair_senders(true, 1).await.unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.channel_posts_total, 3);
    assert_eq!(dry.channel_posts.len(), 2);
    assert_eq!(
        null_senders(&f.url).await.len(),
        5,
        "dry run changes nothing"
    );

    let report = f.store.repair_senders(false, 1).await.unwrap();
    assert_eq!(report.channel_posts_total, 3);
    let per_chat: Vec<_> = report
        .channel_posts
        .iter()
        .map(|c| (c.chat_id, c.rows))
        .collect();
    assert_eq!(per_chat, [(ch2.get(), 1), (ch1.get(), 2)]);
    assert_eq!(
        report
            .unresolved
            .iter()
            .map(|c| (c.chat_id, c.rows))
            .collect::<Vec<_>>(),
        [(grp.get(), 1), (private.get(), 1)]
    );
    assert_eq!(
        null_senders(&f.url).await,
        [(grp.get(), 1), (private.get(), 1)],
        "only private/group rows remain NULL"
    );

    // The deleted post was repaired too, and channel messages now carry the channel.
    let (_, item) = get(&f.app, "/api/v1/chats/-1000000000007/messages/1").await;
    assert_eq!(item["sender_id"], ch1.get());

    let again = f.store.repair_senders(false, 1000).await.unwrap();
    assert_eq!(again.channel_posts_total, 0);
    assert_eq!(again.unresolved_total, 2);
    f.store.close().await;
}

#[tokio::test]
async fn refetch_backfills_null_metadata_without_touching_text_versions_or_deletions() {
    let f = fixture().await;
    let g = group(10);
    let mut edited = msg(g, 2, 20, None);
    edited.text = Some("local edit".into());
    edited.edited_at = DateTime::from_timestamp(BASE + 900, 0);
    let mut kept = msg(g, 3, 30, Some(user(7)));
    kept.post_author = Some("Keep".into());
    let mut with_file = msg(g, 4, 40, None);
    with_file.attachments = vec![Attachment {
        kind: AttachmentKind::Document,
        telegram_file_id: Some("111".into()),
        mime_type: None,
        file_name: None,
        size: None,
    }];
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(g, ChatKind::Group, "G")],
            records: vec![
                record(msg(g, 1, 10, None), MessageSource::History),
                record(edited, MessageSource::Realtime),
                record(kept, MessageSource::History),
                record(with_file, MessageSource::History),
                record(msg(g, 5, 50, None), MessageSource::History),
                deleted(g, 5),
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();

    let fetched = |id: i64, text: &str| {
        let mut m = msg(g, id, 10 * id, Some(user(100 + id)));
        m.text = Some(text.into());
        m.post_author = Some("Sig".into());
        m.forward = Some(Forward {
            from_id: None,
            from_name: Some("Hidden".into()),
            date: DateTime::from_timestamp(BASE - 1, 0),
        });
        m
    };
    let mut file = fetched(4, "ignored");
    file.attachments = vec![Attachment {
        kind: AttachmentKind::Document,
        telegram_file_id: Some("OTHER".into()),
        mime_type: Some("application/pdf".into()),
        file_name: Some("a.pdf".into()),
        size: Some(10),
    }];
    let mut changed_sender = fetched(3, "remote text");
    changed_sender.sender_id = Some(user(999));
    f.store
        .write_batch(IngestBatch {
            records: vec![
                record(fetched(1, "remote text 1"), MessageSource::Refetch),
                // Remote has an older, different version of the edited message.
                record(fetched(2, "stale remote"), MessageSource::Refetch),
                record(changed_sender, MessageSource::Refetch),
                record(file, MessageSource::Refetch),
                record(fetched(5, "zombie"), MessageSource::Refetch),
                // Unknown rows are inserted like a history sync would.
                record(fetched(6, "new row"), MessageSource::Refetch),
            ],
            ..IngestBatch::default()
        })
        .await
        .unwrap();

    let get_message = |id: i64| {
        let store = f.store.clone();
        async move {
            store
                .get(g, MessageId::new(id).unwrap(), true)
                .await
                .unwrap()
                .unwrap()
        }
    };
    let one = get_message(1).await;
    assert_eq!(
        one.text.as_deref(),
        Some("body 1"),
        "text is never replaced"
    );
    assert_eq!(one.sender_id, Some(user(101)), "NULL sender is backfilled");
    assert_eq!(one.post_author.as_deref(), Some("Sig"));
    assert_eq!(
        one.forward.as_ref().unwrap().from_name.as_deref(),
        Some("Hidden")
    );

    let two = get_message(2).await;
    assert_eq!(two.text.as_deref(), Some("local edit"));
    assert_eq!(two.edited_at, DateTime::from_timestamp(BASE + 900, 0));
    assert_eq!(two.sender_id, Some(user(102)));

    let three = get_message(3).await;
    assert_eq!(three.sender_id, Some(user(7)), "non-NULL sender is kept");
    assert_eq!(three.post_author.as_deref(), Some("Keep"));
    assert_eq!(three.text.as_deref(), Some("body 3"));
    assert!(three.forward.is_some(), "NULL forward is still filled");

    let four = get_message(4).await;
    assert_eq!(four.attachments.len(), 1);
    let attachment = &four.attachments[0];
    assert_eq!(attachment.telegram_file_id.as_deref(), Some("111"));
    assert_eq!(attachment.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(attachment.file_name.as_deref(), Some("a.pdf"));
    assert_eq!(attachment.size, Some(10));

    let five = get_message(5).await;
    assert!(five.is_deleted(), "deleted rows stay deleted");
    assert_eq!(five.text.as_deref(), Some("body 5"));
    assert!(
        f.store
            .get(g, MessageId::new(5).unwrap(), false)
            .await
            .unwrap()
            .is_none()
    );

    assert_eq!(get_message(6).await.text.as_deref(), Some("new row"));

    let pool = sqlx::SqlitePool::connect(&f.url).await.unwrap();
    let versions: Vec<(i64, i64, i64)> = sqlx::query_as("SELECT message_id, version_at, source_priority FROM messages WHERE message_id IN (1,2) ORDER BY message_id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(versions, [(1, BASE + 10, 0), (2, BASE + 900, 1)]);
    pool.close().await;
    f.store.close().await;
}

struct Script {
    pages: Mutex<VecDeque<Result<HistoryPage, TelegramError>>>,
    boundaries: Mutex<Vec<Option<i64>>>,
}

#[async_trait]
impl TelegramGateway for Script {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        Ok(vec![])
    }
    async fn fetch_history(
        &self,
        _: ChatId,
        boundary: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        self.boundaries
            .lock()
            .unwrap()
            .push(boundary.before_message_id.map(MessageId::get));
        self.pages
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted page")
    }
}

fn history(chat_id: ChatId, ids: &[i64], next: Option<i64>) -> HistoryPage {
    HistoryPage {
        chats: vec![],
        senders: vec![person(1, "Ann", None)],
        records: ids
            .iter()
            .map(|id| {
                let mut m = msg(chat_id, *id, *id, Some(user(1)));
                m.text = Some("fetched".into());
                record(m, MessageSource::History)
            })
            .collect(),
        next_before_message_id: next.map(|id| MessageId::new(id).unwrap()),
        next_after_message_id: None,
        exhausted: next.is_none(),
    }
}

#[tokio::test]
async fn refetch_walk_is_resumable_and_never_moves_the_normal_checkpoint() {
    let f = fixture().await;
    let g = group(10);
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(g, ChatKind::Group, "G")],
            records: (1..=4)
                .map(|id| record(msg(g, id, id, None), MessageSource::History))
                .collect(),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    let before = f.store.get_checkpoint(g).await.unwrap();

    let script = Arc::new(Script {
        pages: Mutex::new(VecDeque::from([
            Ok(history(g, &[4, 3], Some(3))),
            // Every retry of the second page fails: the job stops with progress kept.
            Err(TelegramError::Unavailable("down".into())),
            Err(TelegramError::Unavailable("down".into())),
            Err(TelegramError::Unavailable("down".into())),
            Err(TelegramError::Unavailable("down".into())),
        ])),
        boundaries: Mutex::default(),
    });
    for job in ["job-1", "job-2"] {
        f.store
            .save_job(SyncJob {
                id: job.into(),
                scope: SyncScope::Chat(g),
                state: SyncJobState::Queued,
                created_at: DateTime::from_timestamp(BASE, 0).unwrap(),
                started_at: None,
                completed_at: None,
                summary_error: None,
            })
            .await
            .unwrap();
    }
    let (sink, _writer) = ingestion_worker::spawn(f.store.clone(), 4);
    let engine = SyncEngine::new(
        script.clone(),
        f.store.clone(),
        f.store.clone(),
        sink,
        PageSize::DEFAULT,
    );
    let cancel = CancellationToken::new();
    assert!(engine.refetch_chat("job-1", g, &cancel).await.is_err());
    let saved = f.store.get_refetch_checkpoint(g).await.unwrap();
    assert!(saved.active);
    assert_eq!(saved.before_id.map(MessageId::get), Some(3));
    assert_eq!(f.store.get_checkpoint(g).await.unwrap(), before);

    script
        .pages
        .lock()
        .unwrap()
        .push_back(Ok(history(g, &[2, 1], None)));
    script.boundaries.lock().unwrap().clear();
    let committed = engine.refetch_chat("job-2", g, &cancel).await.unwrap();
    assert_eq!(committed, 2);
    assert_eq!(
        *script.boundaries.lock().unwrap(),
        [Some(3)],
        "resumed from the saved cursor"
    );
    let done = f.store.get_refetch_checkpoint(g).await.unwrap();
    assert!(!done.active && done.before_id.is_none());

    for id in 1..=4 {
        let message = f
            .store
            .get(g, MessageId::new(id).unwrap(), false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(message.sender_id, Some(user(1)));
        assert_eq!(
            message.text,
            Some(format!("body {id}")),
            "refetch keeps stored text"
        );
    }
    assert_eq!(f.store.get_checkpoint(g).await.unwrap(), before);
    f.store.close().await;
}

#[tokio::test]
async fn migration_0007_upgrades_a_0006_database_without_touching_data() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("old.db").display());
    let old = dir.path().join("old-migrations");
    std::fs::create_dir(&old).unwrap();
    for name in [
        "0001_archive.sql",
        "0002_telegram_account_deletions.sql",
        "0003_chat_tracking.sql",
        "0004_sync_rate_limited_state.sql",
        "0005_read_indexes.sql",
        "0006_search_index.sql",
    ] {
        std::fs::copy(Path::new("migrations").join(name), old.join(name)).unwrap();
    }
    let options = <sqlx::sqlite::SqliteConnectOptions as std::str::FromStr>::from_str(&url)
        .unwrap()
        .create_if_missing(true);
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::migrate::Migrator::new(old)
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    for statement in [
        "INSERT INTO chats(id, kind, title, created_at, updated_at) VALUES (-5, 'group', 'Old', 1, 1)",
        "INSERT INTO chats(id, kind, title, created_at, updated_at) VALUES (-1000000000009, 'channel', 'Chan', 1, 1)",
        "INSERT INTO senders(id, kind, display_name, created_at, updated_at) VALUES (3, 'user', 'Carol', 1, 1)",
        "INSERT INTO messages(chat_id, message_id, sender_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (-5, 1, 3, 100, 100, 1, 1, 'kept', 100, 0)",
        "INSERT INTO messages(chat_id, message_id, sender_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (-1000000000009, 1, NULL, 200, 200, 1, 1, 'legacy post', 200, 0)",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    pool.close().await;

    let store = Arc::new(SqliteStore::connect(&url).await.unwrap());
    let application = Arc::new(Application::new(
        store.clone(),
        store.clone(),
        store.clone(),
        store.clone(),
        None,
        ComponentStatus::new(ComponentState::Running, None),
    ));
    let app = router(application);
    let (status, body) = get(&app, "/api/v1/chats/-5/messages/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["text"], "kept");
    assert_eq!(body["sender"]["display_name"], "Carol");
    assert!(body["post_author"].is_null() && body["forward"].is_null());

    let report = store.repair_senders(false, 1000).await.unwrap();
    assert_eq!(report.channel_posts_total, 1);
    let (_, page) = get(&app, "/api/v1/senders?sort=name").await;
    assert_eq!(sender_ids(&page), [3, -1_000_000_000_009]);
    store.close().await;

    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='index' AND name IN ('messages_sender_stats', 'messages_sender_all_order', 'messages_post_author', 'messages_null_sender') ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(indexes.len(), 4);
    let columns: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('chat_sync_state') WHERE name IN ('refetch_active', 'refetch_before_id')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(columns, 2);
    pool.close().await;
}

fn cli(url: &str, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(BIN)
        .env_clear()
        .env("TGARCHIVE_NO_DOTENV", "1")
        .env("DATABASE_URL", url)
        .args(args)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[tokio::test]
async fn cli_resolves_senders_by_id_username_name_me_and_reports_ambiguity() {
    let f = fixture().await;
    seed_senders(&f).await;
    f.store.close().await;
    let url = f.url.clone();
    let _keep = &f.dir;

    let (ok, out, _) = cli(&url, &["--output", "json", "senders", "search", "小明"]);
    assert!(ok);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json[0]["id"], 1);
    assert_eq!(json[0]["message_count"], 4);

    let (ok, out, _) = cli(&url, &["--output", "json", "senders", "get", "@alice"]);
    assert!(ok);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["id"], 2);
    assert_eq!(json["chats"][0]["chat_id"], -10);

    let (ok, out, _) = cli(&url, &["senders", "get", "me"]);
    assert!(ok, "{out}");
    assert!(out.contains("self") && out.contains("Me"));

    let (ok, out, _) = cli(
        &url,
        &["--output", "json", "messages", "list", "--sender", "me"],
    );
    assert!(ok);
    let json: Value = serde_json::from_str(&out).unwrap();
    let ids: Vec<_> = json["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["sender_id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [9, 9]);

    let (ok, out, _) = cli(
        &url,
        &[
            "--output",
            "json",
            "messages",
            "search",
            "body",
            "--sender",
            "王小明",
        ],
    );
    assert!(ok, "{out}");
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["items"].as_array().unwrap().len(), 4);

    let (ok, _, err) = cli(&url, &["messages", "list", "--sender", "alice"]);
    assert!(!ok, "two senders match 'alice'");
    assert!(
        err.contains("ambiguous") && err.contains("Alice Chen") && err.contains("Alice Wu"),
        "{err}"
    );

    let (ok, _, err) = cli(&url, &["messages", "list", "--sender", "zzz"]);
    assert!(!ok);
    assert!(err.contains("no sender matches"), "{err}");

    let (ok, out, _) = cli(&url, &["messages", "list", "--sender", "3"]);
    assert!(ok);
    assert_eq!(out.lines().filter(|l| l.contains("body")).count(), 2);

    let (ok, _, err) = cli(
        &url,
        &["messages", "list", "--sender", "1", "--sender-id", "2"],
    );
    assert!(!ok, "{err}");
}

#[tokio::test]
async fn cli_repair_senders_supports_dry_run_and_is_idempotent() {
    let f = fixture().await;
    let ch = channel(7);
    f.store
        .write_batch(IngestBatch {
            chats: vec![chat(ch, ChatKind::Channel, "News")],
            records: vec![record(msg(ch, 1, 1, None), MessageSource::History)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    f.store.close().await;

    let (ok, out, _) = cli(&f.url, &["repair", "senders", "--dry-run"]);
    assert!(ok);
    assert!(out.contains("would fix 1 rows"), "{out}");
    assert_eq!(null_senders(&f.url).await.len(), 1);

    let (ok, out, _) = cli(&f.url, &["--output", "json", "repair", "senders"]);
    assert!(ok);
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["channel_posts_total"], 1);
    assert!(null_senders(&f.url).await.is_empty());

    let (ok, out, _) = cli(&f.url, &["repair", "senders"]);
    assert!(ok);
    assert!(out.contains("fixed 0 rows"), "{out}");
}

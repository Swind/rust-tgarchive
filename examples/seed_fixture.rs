//! Dev-only: builds a deterministic fixture archive for the browser E2E tests, without Telegram.
//!
//!     cargo run --example seed_fixture -- <db path>
//!
//! Everything goes through the real store APIs; all message/job timestamps are fixed constants.
//! (An example, not a CLI subcommand, so the shipped binary carries no fixture code.)

use chrono::{DateTime, Duration, TimeZone, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tgarchive::{
    application::{
        AccountDeletion, ArchiveWriter, ChatCheckpoint, ChatRepository, IngestBatch, IngestRecord,
        MessageSource, SyncChatProgress, SyncJob, SyncJobState, SyncRepository, SyncScope,
    },
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId,
        Sender, SenderId, SenderKind,
    },
    infrastructure::persistence::sqlite::SqliteStore,
};

const ALICE: i64 = 1001;
const BOB: i64 = 2002;
const CAROL: i64 = 3003;
const TELEGRAM: i64 = 777_000;
const BIG: i64 = -1_002_000_001;
const NEWS: i64 = -1_002_000_002;
const EMPTY: i64 = -1_002_000_003;
const FAMILY: i64 = -300_001;
const OLD: i64 = -300_002;

fn base() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 3, 1, 2, 0, 0).unwrap()
}

fn cid(id: i64) -> ChatId {
    ChatId::from_marked(id).unwrap()
}

fn mid(id: i64) -> MessageId {
    MessageId::new(id).unwrap()
}

fn chat(id: i64, kind: ChatKind, title: &str, username: Option<&str>) -> Chat {
    Chat {
        id: cid(id),
        kind,
        title: Some(title.into()),
        username: username.map(Into::into),
        tracked: false,
    }
}

fn sender(id: i64, name: &str, username: Option<&str>) -> Sender {
    Sender {
        id: SenderId::from_marked(id).unwrap(),
        kind: SenderKind::User,
        display_name: Some(name.into()),
        username: username.map(Into::into),
    }
}

fn msg(chat: i64, id: i64, sender: Option<i64>, at: DateTime<Utc>, text: Option<&str>) -> Message {
    Message {
        id: mid(id),
        chat_id: cid(chat),
        sender_id: sender.map(|s| SenderId::from_marked(s).unwrap()),
        timestamp: at,
        edited_at: None,
        collected_at: at,
        text: text.map(Into::into),
        reply_to: None,
        attachments: Vec::new(),
    }
}

fn attachment(
    kind: AttachmentKind,
    name: Option<&str>,
    mime: Option<&str>,
    size: i64,
) -> Attachment {
    Attachment {
        kind,
        telegram_file_id: Some(format!("fixture-{kind:?}")),
        mime_type: mime.map(Into::into),
        file_name: name.map(Into::into),
        size: Some(size),
    }
}

fn created(m: Message) -> IngestRecord {
    IngestRecord {
        event: MessageEvent::Created(m),
        source: MessageSource::History,
    }
}

fn big_chat_messages() -> Vec<Message> {
    let who = [ALICE, BOB, CAROL];
    let names = ["愛麗絲", "Bob", "Carol"];
    let mut out = Vec::new();
    for id in 1..=260i64 {
        let at = base() + Duration::days((id - 1) / 30) + Duration::minutes(((id - 1) % 30) * 10);
        let s = who[(id % 3) as usize];
        let mut text = format!("第 {id} 則訊息，來自 {}", names[(id % 3) as usize]);
        match id {
            50 | 150 | 250 => text.push_str(" — remember to check kubernetes rollout"),
            120 | 121 => text.push_str(" 請看 測試關鍵字 的說明"),
            60 => text = "ephemeral note that will vanish".into(),
            61 => text = "另一則會被刪除的訊息".into(),
            100 => text = "這是編輯後的版本（已更正）".into(),
            _ => {}
        }
        let mut m = msg(BIG, id, Some(s), at, Some(&text));
        match id {
            100 => m.edited_at = Some(at + Duration::minutes(3)),
            101 | 205 => m.reply_to = Some(mid(100)),
            30 => m.attachments = vec![attachment(AttachmentKind::Photo, None, None, 204_800)],
            31 => {
                m.attachments = vec![attachment(
                    AttachmentKind::Video,
                    Some("demo.mp4"),
                    Some("video/mp4"),
                    8_388_608,
                )]
            }
            32 => {
                m.attachments = vec![attachment(
                    AttachmentKind::Audio,
                    Some("song.mp3"),
                    Some("audio/mpeg"),
                    3_145_728,
                )]
            }
            33 => {
                m.attachments = vec![attachment(
                    AttachmentKind::Voice,
                    None,
                    Some("audio/ogg"),
                    20_480,
                )]
            }
            34 => {
                m.attachments = vec![attachment(
                    AttachmentKind::Document,
                    Some("report.pdf"),
                    Some("application/pdf"),
                    1_572_864,
                )]
            }
            35 => {
                m.attachments = vec![attachment(
                    AttachmentKind::Sticker,
                    None,
                    Some("image/webp"),
                    30_000,
                )]
            }
            36 => {
                m.attachments = vec![attachment(
                    AttachmentKind::Animation,
                    Some("cat.gif"),
                    Some("image/gif"),
                    900_000,
                )]
            }
            37 => m.attachments = vec![attachment(AttachmentKind::Other, None, None, 10)],
            38 => {
                m.attachments = vec![
                    attachment(AttachmentKind::Photo, None, None, 100_000),
                    attachment(
                        AttachmentKind::Document,
                        Some("notes.txt"),
                        Some("text/plain"),
                        512,
                    ),
                ]
            }
            259 => {
                // Service message (e.g. group action): no text and no attachments.
                m.text = None;
                m.sender_id = None;
            }
            _ => {}
        }
        out.push(m);
    }
    out
}

fn progress(
    job: &str,
    chat: i64,
    state: SyncJobState,
    count: u64,
    err: Option<&str>,
) -> SyncChatProgress {
    SyncChatProgress {
        job_id: job.into(),
        chat_id: cid(chat),
        state,
        committed_messages: count,
        summary_error: err.map(Into::into),
    }
}

async fn job(
    store: &SqliteStore,
    id: &str,
    scope: SyncScope,
    state: SyncJobState,
    hour: i64,
    err: Option<&str>,
    chats: Vec<SyncChatProgress>,
) {
    let created_at = Utc.with_ymd_and_hms(2026, 3, 11, 0, 0, 0).unwrap() + Duration::hours(hour);
    let mut j = SyncJob {
        id: id.into(),
        scope,
        state: SyncJobState::Running,
        created_at,
        started_at: Some(created_at + Duration::seconds(1)),
        completed_at: None,
        summary_error: None,
    };
    SyncRepository::save_job(store, j.clone()).await.unwrap();
    for p in chats {
        store
            .write_batch(IngestBatch {
                job_progress: Some(p),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    j.state = state;
    j.completed_at = Some(created_at + Duration::minutes(5));
    j.summary_error = err.map(Into::into);
    SyncRepository::save_job(store, j).await.unwrap();
}

#[tokio::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: seed_fixture <db path>");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{path}{suffix}"));
    }
    let url = format!("sqlite://{path}");
    let store = SqliteStore::connect(&url).await.unwrap();

    let chats = vec![
        chat(ALICE, ChatKind::Private, "Alice 愛麗絲", Some("alice_w")),
        chat(TELEGRAM, ChatKind::Private, "Telegram", None),
        chat(BIG, ChatKind::Supergroup, "Rust 台灣社群", Some("rust_tw")),
        chat(
            NEWS,
            ChatKind::Channel,
            "Daily News 每日快訊",
            Some("daily_news"),
        ),
        chat(FAMILY, ChatKind::Group, "家庭群組", None),
        chat(OLD, ChatKind::Group, "舊專案群組", None),
        chat(EMPTY, ChatKind::Supergroup, "Empty Lab 空聊天室", None),
    ];
    let senders = vec![
        sender(ALICE, "Alice 愛麗絲", Some("alice_w")),
        sender(BOB, "Bob", None),
        sender(CAROL, "Carol Chen", Some("carol_c")),
    ];
    store
        .write_batch(IngestBatch {
            chats,
            senders,
            ..Default::default()
        })
        .await
        .unwrap();

    let mut records: Vec<IngestRecord> = big_chat_messages().into_iter().map(created).collect();
    // Alice's private chat.
    for i in 1..=8i64 {
        let at = base() + Duration::days(i) + Duration::hours(1);
        let from = if i % 2 == 0 { ALICE } else { BOB };
        records.push(created(msg(
            ALICE,
            i,
            Some(from),
            at,
            Some(&format!("私訊 {i}：hello from {from}")),
        )));
    }
    // Service account: sender == chat, no senders row.
    for (i, t) in ["Login code: 12345. Do not share it.", "新的登入裝置通知"]
        .iter()
        .enumerate()
    {
        let at = base() + Duration::days(2 + i as i64);
        records.push(created(msg(
            TELEGRAM,
            1 + i as i64,
            Some(TELEGRAM),
            at,
            Some(t),
        )));
    }
    // Channel posts have no sender row either.
    for i in 1..=12i64 {
        let at = base() + Duration::days(i % 6) + Duration::minutes(i);
        let mut m = msg(
            NEWS,
            i,
            Some(NEWS),
            at,
            Some(&format!("頭條 {i}: kubernetes weekly digest #{i}")),
        );
        if i % 4 == 0 {
            m.attachments = vec![attachment(AttachmentKind::Photo, None, None, 150_000)];
        }
        records.push(created(m));
    }
    // Family group: starts with a service message (group created).
    records.push(created(msg(FAMILY, 1, Some(ALICE), base(), None)));
    for i in 2..=15i64 {
        let at = base() + Duration::days(i / 3) + Duration::minutes(i);
        records.push(created(msg(
            FAMILY,
            i,
            Some(if i % 2 == 0 { ALICE } else { BOB }),
            at,
            Some(&format!("家庭訊息 {i}")),
        )));
    }
    // Search evaluation set (Traditional Chinese, mixed language, fullwidth, and a text that has
    // the words 台北 / 咖啡 but never the contiguous character sequence 北咖啡).
    for (i, text) in [
        "今天下午要去台北喝咖啡",
        "台北咖啡廳的拿鐵很好喝",
        "伺服器昨晚又當機了",
        "硬碟壞軌需要更換",
        "GitLab Runner 沒有回應",
        "Chromium 瀏覽器更新了",
        "今天研究 ＳＱＬｉｔｅ 全文檢索",
        "台北車站附近的咖啡很貴",
        "我在新北咖啡店工作",
        "長訊息：這是一段很長的內容，只是用來測試摘要。這是一段很長的內容，只是用來測試摘要。這是一段很長的內容，只是用來測試摘要。這是一段很長的內容，只是用來測試摘要。最後提到硬碟故障的原因。這是一段很長的內容，只是用來測試摘要。這是一段很長的內容，只是用來測試摘要。",
    ]
    .iter()
    .enumerate()
    {
        let at = base() - Duration::days(30) + Duration::hours(i as i64);
        records.push(created(msg(OLD, 1 + i as i64, Some(BOB), at, Some(text))));
    }
    // Deleted messages in the big chat (created above, then tombstoned).
    for id in [60, 61] {
        records.push(IngestRecord {
            event: MessageEvent::Deleted {
                chat_id: cid(BIG),
                message_id: mid(id),
                deleted_at: base() + Duration::days(9),
            },
            source: MessageSource::Realtime,
        });
    }
    for chunk in records.chunks(100) {
        store
            .write_batch(IngestBatch {
                records: chunk.to_vec(),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    for id in [ALICE, BIG, NEWS, FAMILY, OLD] {
        ChatRepository::set_tracked(&store, cid(id), true)
            .await
            .unwrap();
    }
    for id in [ALICE, BIG, NEWS, FAMILY] {
        store
            .write_batch(IngestBatch {
                checkpoint: Some((
                    cid(id),
                    ChatCheckpoint {
                        history_before_id: None,
                        history_complete: true,
                        catchup_after_id: None,
                    },
                )),
                ..Default::default()
            })
            .await
            .unwrap();
    }
    store
        .write_batch(IngestBatch {
            chat_error: Some((cid(OLD), "Telegram rate limit: retry after 3600 s".into())),
            ..Default::default()
        })
        .await
        .unwrap();

    // One account-wide deletion that matches no archived message: counted as unresolved.
    store
        .bind_telegram_account(SenderId::from_marked(9000).unwrap())
        .await
        .unwrap();
    store
        .write_batch(IngestBatch {
            account_deletions: vec![AccountDeletion {
                message_id: mid(999_999),
                deleted_at: base() + Duration::days(9),
            }],
            ..Default::default()
        })
        .await
        .unwrap();

    use SyncJobState::*;
    job(
        &store,
        "job-succeeded",
        SyncScope::All,
        Succeeded,
        1,
        None,
        vec![
            progress("job-succeeded", BIG, Succeeded, 260, None),
            progress("job-succeeded", NEWS, Succeeded, 12, None),
        ],
    )
    .await;
    job(
        &store,
        "job-failed",
        SyncScope::Chat(cid(OLD)),
        Failed,
        2,
        Some("Telegram unavailable: connection reset by peer"),
        vec![progress(
            "job-failed",
            OLD,
            Failed,
            0,
            Some("Telegram unavailable: connection reset by peer"),
        )],
    )
    .await;
    job(
        &store,
        "job-rate-limited",
        SyncScope::All,
        RateLimited,
        3,
        Some("Telegram rate limit: retry after 3600 s"),
        vec![
            progress("job-rate-limited", FAMILY, Succeeded, 14, None),
            progress(
                "job-rate-limited",
                OLD,
                RateLimited,
                40,
                Some("Telegram rate limit: retry after 3600 s"),
            ),
        ],
    )
    .await;
    job(
        &store,
        "job-interrupted",
        SyncScope::Chat(cid(ALICE)),
        Interrupted,
        4,
        Some("process stopped before job completed"),
        vec![progress(
            "job-interrupted",
            ALICE,
            Interrupted,
            3,
            Some("process stopped before chat sync completed"),
        )],
    )
    .await;
    // Full-text index: maintained on write; rebuild once to prove the batch path and fail loudly
    // if the fixture would be searched through the LIKE fallback.
    store.rebuild_search_index(1000, |_| {}).await.unwrap();
    assert!(store.search_index_ready().await.unwrap());
    store.close().await;

    // Pin the sync timestamps (the store stamps them with wall-clock time).
    let pool: SqlitePool = SqlitePoolOptions::new()
        .connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    let done = Utc
        .with_ymd_and_hms(2026, 3, 11, 1, 5, 0)
        .unwrap()
        .timestamp();
    sqlx::query("UPDATE chat_sync_state SET last_sync_completed_at=?, last_sync_started_at=? WHERE last_sync_completed_at IS NOT NULL")
        .bind(done)
        .bind(done - 300)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    println!("fixture written to {path}");
}

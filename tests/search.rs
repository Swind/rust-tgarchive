//! Chinese-friendly full-text search (jieba words + CJK bigrams in a contentless FTS5 table).

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::{SqlitePool, migrate::Migrator};
use tgarchive::{
    application::{
        ArchiveWriter, IngestBatch, IngestRecord, MessageCursor, MessageFilters, MessagePage,
        MessageRepository, MessageSource, MessageView, PageSize, SearchIndexState,
        SearchMessagesQuery, SearchSort, TimeRange,
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageEvent, MessageId, SenderId},
    infrastructure::persistence::sqlite::SqliteStore,
};

const BASE: i64 = 1_700_000_000;

async fn store() -> (tempfile::TempDir, SqliteStore, String) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("archive.db").display());
    let store = SqliteStore::connect(&url).await.unwrap();
    (dir, store, url)
}

fn chat_id(id: i64) -> ChatId {
    ChatId::from_marked(id).unwrap()
}

fn chat(id: i64) -> Chat {
    Chat {
        id: chat_id(id),
        kind: ChatKind::Group,
        title: Some(format!("chat {id}")),
        username: None,
        tracked: false,
    }
}

fn message(chat: i64, id: i64, text: &str) -> Message {
    let time = DateTime::<Utc>::from_timestamp(BASE + id, 0).unwrap();
    Message {
        post_author: None,
        forward: None,
        id: MessageId::new(id).unwrap(),
        chat_id: chat_id(chat),
        sender_id: None,
        timestamp: time,
        edited_at: None,
        collected_at: time,
        text: Some(text.to_owned()),
        reply_to: None,
        attachments: Vec::new(),
    }
}

fn record(event: MessageEvent, source: MessageSource) -> IngestRecord {
    IngestRecord { event, source }
}

async fn add(store: &SqliteStore, messages: Vec<Message>) {
    let chats: Vec<Chat> = messages
        .iter()
        .map(|m| m.chat_id.get())
        .collect::<HashSet<_>>()
        .into_iter()
        .map(chat)
        .collect();
    store
        .write_batch(IngestBatch {
            chats,
            records: messages
                .into_iter()
                .map(|m| record(MessageEvent::Created(m), MessageSource::History))
                .collect(),
            ..IngestBatch::default()
        })
        .await
        .unwrap();
}

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

fn query(text: &str) -> SearchMessagesQuery {
    SearchMessagesQuery {
        text: text.into(),
        filters: filters(),
        before: None,
        after: None,
        page_size: PageSize::new(100).unwrap(),
        sort: SearchSort::Relevance,
        offset: 0,
    }
}

async fn ids(store: &SqliteStore, text: &str) -> Vec<i64> {
    ids_of(store.search(query(text)).await.unwrap())
}

fn ids_of(page: MessagePage) -> Vec<i64> {
    let mut ids: Vec<i64> = page.items.iter().map(|m| m.id.get()).collect();
    ids.sort_unstable();
    ids
}

async fn raw(url: &str) -> SqlitePool {
    SqlitePool::connect(url).await.unwrap()
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

fn text_of(view: &MessageView) -> &str {
    view.text.as_deref().unwrap()
}

#[tokio::test]
async fn evaluation_set_finds_expected_messages() {
    let (_dir, store, _) = store().await;
    add(
        &store,
        vec![
            message(10, 1, "今天去台北喝咖啡"),
            message(10, 2, "GitLab Runner 沒有回應"),
            message(10, 3, "Chromium build failed"),
            message(10, 4, "台中今天下雨"),
            message(10, 5, "伺服器又當機了，重開機"),
            message(10, 6, "硬碟壞軌需要更換"),
            message(10, 7, "今天研究 ＳＱＬｉｔｅ 的全文檢索"),
            message(10, 8, "台北有很多好喝的咖啡"),
            message(10, 9, "台灣的北部很多咖啡館，啡色的牆"),
        ],
    )
    .await;
    assert_eq!(ids(&store, "台北").await, [1, 8]);
    assert_eq!(ids(&store, "咖啡").await, [1, 8, 9]);
    assert_eq!(
        ids(&store, "台北咖啡").await,
        [1, 8],
        "words OR contiguous bigrams"
    );
    assert_eq!(ids(&store, "GitLab Runner").await, [2]);
    assert_eq!(ids(&store, "gitlab").await, [2]);
    assert_eq!(ids(&store, "chromium").await, [3]);
    assert_eq!(ids(&store, "伺服器").await, [5]);
    assert_eq!(ids(&store, "硬碟").await, [6]);
    assert_eq!(
        ids(&store, "sqlite").await,
        [7],
        "fullwidth text is normalized"
    );
    assert_eq!(ids(&store, "ＳＱＬｉｔｅ").await, [7]);
    assert_eq!(ids(&store, "gitlab 沒有回應").await, [2], "mixed query");
}

#[tokio::test]
async fn ascii_terms_match_by_prefix() {
    let (_dir, store, _) = store().await;
    add(
        &store,
        vec![
            message(10, 1, "Some benchmarks of the GitLab runner"),
            message(10, 2, "abc def"),
            message(10, 3, "ab only"),
            message(10, 4, "台北 benchmarks 咖啡"),
        ],
    )
    .await;
    assert_eq!(ids(&store, "benchmark").await, [1, 4]);
    assert_eq!(ids(&store, "gitl").await, [1]);
    assert_eq!(ids(&store, "ab").await, [3], "two characters stay exact");
    assert_eq!(ids(&store, "benchmark 咖啡").await, [4]);
    for q in [
        "bench*",
        "ben\"",
        "a:b",
        "NEAR(ben",
        "bench OR",
        "-ben",
        "\"bench\"*",
    ] {
        ids(&store, q).await;
    }
    assert!(ids(&store, "bench*").await.len() <= 2);
}

#[tokio::test]
async fn bigram_phrase_is_contiguous_not_scattered() {
    let (_dir, store, url) = store().await;
    add(
        &store,
        vec![
            message(10, 1, "台北咖啡"),
            // Bigrams 台北, 北咖, 咖啡 are all present but never adjacent to each other.
            message(10, 2, "台北新北咖啡"),
            message(10, 3, "台北好朋友北咖哩飯咖啡"),
            message(10, 4, "台北咖哩"),
        ],
    )
    .await;
    let pool = raw(&url).await;
    // The bigram path of the plan for 台北咖啡, run on its own against the index.
    let phrase: Vec<i64> = sqlx::query_scalar(
        "SELECT rowid FROM messages_fts WHERE messages_fts MATCH 'bigrams:\"台北 北咖 咖啡\"' ORDER BY rowid",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        phrase.len(),
        1,
        "only the contiguous text matches the phrase"
    );
    // An AND over the same bigrams would (wrongly) accept the scattered texts.
    let and: Vec<i64> = sqlx::query_scalar(
        "SELECT rowid FROM messages_fts WHERE messages_fts MATCH 'bigrams:\"台北\" AND bigrams:\"北咖\" AND bigrams:\"咖啡\"'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(and.len() > phrase.len());
    // End to end: text lacking the words and the contiguous sequence is not returned.
    let found = ids(&store, "台北咖啡").await;
    assert!(found.contains(&1));
    assert!(!found.contains(&4), "{found:?}");
}

#[tokio::test]
async fn bigram_fallback_finds_text_jieba_segments_differently() {
    let (_dir, store, _) = store().await;
    // A substring that straddles jieba word boundaries.
    add(&store, vec![message(10, 1, "他說今天下午要去台北喝咖啡")]).await;
    assert_eq!(ids(&store, "午要去台").await, [1]);
    assert_eq!(ids(&store, "北喝咖").await, [1]);
}

#[tokio::test]
async fn single_character_query_scans_text() {
    let (_dir, store, _) = store().await;
    add(
        &store,
        vec![
            message(10, 1, "台北"),
            message(10, 2, "高雄"),
            message(10, 3, "北方"),
        ],
    )
    .await;
    assert_eq!(
        ids(&store, "北").await,
        [1, 3],
        "北 at the end of 台北 is found"
    );
    assert_eq!(ids(&store, "的").await, Vec::<i64>::new());
    let page = store.search(query("北")).await.unwrap();
    assert!(page.items.iter().all(|m| text_of(m).contains('北')));
    assert!(page.items[0].snippet.is_some());
}

#[tokio::test]
async fn like_metacharacters_are_literal_in_fallback_scans() {
    let (_dir, store, _) = store().await;
    add(
        &store,
        vec![message(10, 1, "100% done"), message(10, 2, "100x done")],
    )
    .await;
    // "%" alone has no letter/digit: rejected before it could act as a wildcard.
    assert!(query("%").validate().is_err());
    assert_eq!(ids(&store, "100%").await, [1]);
}

#[tokio::test]
async fn soft_delete_keeps_the_index_entry_and_honors_include_deleted() {
    let (_dir, store, url) = store().await;
    add(
        &store,
        vec![
            message(10, 1, "今天去台北喝咖啡"),
            message(10, 2, "台北車站"),
        ],
    )
    .await;
    store
        .write_batch(IngestBatch {
            records: vec![record(
                MessageEvent::Deleted {
                    chat_id: chat_id(10),
                    message_id: MessageId::new(1).unwrap(),
                    deleted_at: DateTime::from_timestamp(BASE + 100, 0).unwrap(),
                },
                MessageSource::Realtime,
            )],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    assert_eq!(ids(&store, "台北").await, [2]);
    for sort in [SearchSort::Relevance, SearchSort::Time] {
        let mut with_deleted = query("台北");
        with_deleted.sort = sort;
        with_deleted.filters.include_deleted = true;
        let page = store.search(with_deleted).await.unwrap();
        assert_eq!(ids_of(page.clone()), [1, 2]);
        assert!(page.items.iter().any(|m| m.is_deleted()));
        let mut single = query("北");
        single.sort = sort;
        single.filters.include_deleted = true;
        assert_eq!(ids_of(store.search(single).await.unwrap()), [1, 2]);
    }
    // The entry was never removed.
    let pool = raw(&url).await;
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM messages_fts_docsize").await,
        2
    );
}

#[tokio::test]
async fn stale_history_upsert_keeps_index_and_newer_realtime_edit_reindexes() {
    let (_dir, store, _) = store().await;
    let mut original = message(10, 1, "今天去台北");
    original.edited_at = Some(DateTime::from_timestamp(BASE + 50, 0).unwrap());
    store
        .write_batch(IngestBatch {
            chats: vec![chat(10)],
            records: vec![record(
                MessageEvent::Created(original),
                MessageSource::Realtime,
            )],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    assert_eq!(ids(&store, "台北").await, [1]);

    // Older history copy (version_at older than the stored edit) must not touch the index.
    let mut stale = message(10, 1, "今天去高雄");
    stale.edited_at = Some(DateTime::from_timestamp(BASE + 10, 0).unwrap());
    store
        .write_batch(IngestBatch {
            records: vec![record(MessageEvent::Updated(stale), MessageSource::History)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    assert_eq!(ids(&store, "台北").await, [1]);
    assert!(ids(&store, "高雄").await.is_empty());

    // Newer realtime edit re-indexes: old term gone, new term found.
    let mut edit = message(10, 1, "今天去高雄");
    edit.edited_at = Some(DateTime::from_timestamp(BASE + 90, 0).unwrap());
    store
        .write_batch(IngestBatch {
            records: vec![record(MessageEvent::Updated(edit), MessageSource::Realtime)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    assert!(ids(&store, "台北").await.is_empty());
    assert_eq!(ids(&store, "高雄").await, [1]);

    // Edited to empty text: removed from the index.
    let mut cleared = message(10, 1, "");
    cleared.text = None;
    cleared.edited_at = Some(DateTime::from_timestamp(BASE + 95, 0).unwrap());
    store
        .write_batch(IngestBatch {
            records: vec![record(
                MessageEvent::Updated(cleared),
                MessageSource::Realtime,
            )],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    assert!(ids(&store, "高雄").await.is_empty());
    let status = store.search_index_status().await.unwrap();
    assert_eq!((status.indexed, status.total), (0, 0));
}

#[tokio::test]
async fn unchanged_text_is_not_reindexed() {
    let (_dir, store, url) = store().await;
    add(&store, vec![message(10, 1, "台北咖啡")]).await;
    let pool = raw(&url).await;
    let before = count(&pool, "SELECT COUNT(*) FROM messages_fts_data").await;
    let mut same = message(10, 1, "台北咖啡");
    same.edited_at = Some(DateTime::from_timestamp(BASE + 5, 0).unwrap());
    store
        .write_batch(IngestBatch {
            records: vec![record(MessageEvent::Updated(same), MessageSource::Realtime)],
            ..IngestBatch::default()
        })
        .await
        .unwrap();
    assert_eq!(ids(&store, "台北").await, [1]);
    // Cheap proxy: the index segment table did not grow from a rewrite.
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM messages_fts_data").await,
        before
    );
}

#[tokio::test]
async fn edit_and_physical_delete_update_both_columns() {
    let (_dir, store, url) = store().await;
    add(&store, vec![message(10, 1, "今天去台北")]).await;
    assert_eq!(ids(&store, "台北").await, [1]);
    let pool = raw(&url).await;
    sqlx::query(
        "DELETE FROM messages_fts WHERE rowid=(SELECT row_id FROM messages WHERE message_id=1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM messages WHERE message_id=1")
        .execute(&pool)
        .await
        .unwrap();
    assert!(ids(&store, "台北").await.is_empty());
    assert!(ids(&store, "北").await.is_empty());
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM messages").await, 0);
}

#[tokio::test]
async fn index_write_failure_rolls_back_the_message() {
    let (_dir, store, url) = store().await;
    let pool = raw(&url).await;
    sqlx::query("DROP TABLE messages_fts")
        .execute(&pool)
        .await
        .unwrap();
    let result = store
        .write_batch(IngestBatch {
            chats: vec![chat(10)],
            records: vec![record(
                MessageEvent::Created(message(10, 1, "台北")),
                MessageSource::History,
            )],
            ..IngestBatch::default()
        })
        .await;
    assert!(result.is_err());
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM messages").await, 0);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM chats").await, 0);
}

#[tokio::test]
async fn fts_table_is_contentless_and_has_no_triggers() {
    let (_dir, store, url) = store().await;
    add(&store, vec![message(10, 1, "今天去台北喝咖啡")]).await;
    let pool = raw(&url).await;
    let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='messages_fts'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        sql.contains("content=''") && sql.contains("contentless_delete=1"),
        "{sql}"
    );
    let words: Option<String> = sqlx::query_scalar("SELECT words FROM messages_fts WHERE rowid=1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        words, None,
        "contentless tables return NULL for stored columns"
    );
    let bigrams: Option<String> =
        sqlx::query_scalar("SELECT bigrams FROM messages_fts WHERE rowid=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bigrams, None);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'"
        )
        .await,
        0
    );
    let status = store.search_index_status().await.unwrap();
    assert_eq!(status.state, SearchIndexState::Ready);
    assert_eq!((status.version, status.indexed, status.total), (1, 1, 1));
}

#[tokio::test]
async fn relevance_pagination_has_no_duplicates_or_gaps() {
    let (_dir, store, _) = store().await;
    let mut messages = Vec::new();
    for id in 1..=23 {
        // Varying length and repetition spreads bm25 scores; some ties.
        let filler = "字".repeat((id % 5) as usize * 3);
        let repeat = "咖啡 ".repeat((id % 3 + 1) as usize);
        messages.push(message(10, id, &format!("台北 {repeat}{filler}")));
    }
    add(&store, messages).await;
    for sort in [SearchSort::Relevance, SearchSort::Time] {
        let mut seen = Vec::new();
        let mut offset = 0;
        let mut cursor: Option<MessageCursor> = None;
        loop {
            let mut q = query("台北咖啡");
            q.page_size = PageSize::new(7).unwrap();
            q.sort = sort;
            q.offset = offset;
            q.before = cursor.clone();
            let page = store.search(q).await.unwrap();
            seen.extend(page.items.iter().map(|m| m.id.get()));
            if !page.has_more {
                break;
            }
            match sort {
                SearchSort::Relevance => offset = page.next_offset.expect("offset cursor"),
                SearchSort::Time => cursor = Some(page.next_cursor.expect("keyset cursor")),
            }
        }
        let unique: HashSet<_> = seen.iter().copied().collect();
        assert_eq!(seen.len(), 23, "{sort:?}: {seen:?}");
        assert_eq!(unique.len(), 23, "{sort:?} has duplicates: {seen:?}");
    }
    // Relevance really orders by relevance: more occurrences rank higher than fewer.
    let page = store.search(query("咖啡")).await.unwrap();
    assert_eq!(page.items.len(), 23);
    let top = text_of(&page.items[0]).matches("咖啡").count();
    let bottom = text_of(page.items.last().unwrap()).matches("咖啡").count();
    assert!(top >= bottom, "{top} < {bottom}");
}

#[tokio::test]
async fn filters_work_for_both_sorts_and_like_paths() {
    let (_dir, store, _) = store().await;
    let mut a = message(10, 1, "台北咖啡");
    a.sender_id = Some(SenderId::from_marked(5).unwrap());
    let b = message(20, 2, "台北咖啡廳");
    let c = message(10, 3, "台北咖啡 很晚");
    add(&store, vec![a, b, c]).await;
    for sort in [SearchSort::Relevance, SearchSort::Time] {
        for text in ["台北咖啡", "北"] {
            let mut q = query(text);
            q.sort = sort;
            q.filters.chat_id = Some(chat_id(20));
            assert_eq!(
                ids_of(store.search(q).await.unwrap()),
                [2],
                "{sort:?} {text}"
            );
            let mut q = query(text);
            q.sort = sort;
            q.filters.sender_id = Some(SenderId::from_marked(5).unwrap());
            assert_eq!(
                ids_of(store.search(q).await.unwrap()),
                [1],
                "{sort:?} {text}"
            );
            let mut q = query(text);
            q.sort = sort;
            q.filters.time_range = TimeRange::new(
                DateTime::from_timestamp(BASE + 3, 0),
                DateTime::from_timestamp(BASE + 10, 0),
            )
            .unwrap();
            assert_eq!(
                ids_of(store.search(q).await.unwrap()),
                [3],
                "{sort:?} {text}"
            );
        }
    }
}

#[tokio::test]
async fn snippets_surround_the_first_match() {
    let (_dir, store, _) = store().await;
    let long = format!("{}台北咖啡{}", "前".repeat(200), "後".repeat(200));
    add(
        &store,
        vec![message(10, 1, &long), message(10, 2, "台北咖啡")],
    )
    .await;
    let page = store.search(query("台北咖啡")).await.unwrap();
    for item in &page.items {
        let snippet = item.snippet.as_deref().expect("snippet");
        assert!(snippet.contains("台北咖啡"));
        assert!(
            snippet.chars().count() <= 122,
            "{}",
            snippet.chars().count()
        );
    }
    let short = page.items.iter().find(|m| m.id.get() == 2).unwrap();
    assert_eq!(short.snippet.as_deref(), Some("台北咖啡"));
}

#[tokio::test]
async fn not_ready_index_falls_back_to_like_and_rebuild_restores_search() {
    let (_dir, store, url) = store().await;
    add(
        &store,
        vec![
            message(10, 1, "今天去台北喝咖啡"),
            message(10, 2, "GitLab Runner"),
        ],
    )
    .await;
    let pool = raw(&url).await;
    // Simulate an outdated index: wrong version.
    sqlx::query("UPDATE app_metadata SET value='0' WHERE key='search_index_version'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.search_index_status().await.unwrap().state,
        SearchIndexState::Stale
    );
    sqlx::query("DELETE FROM messages_fts")
        .execute(&pool)
        .await
        .unwrap();
    // The (now empty) index is not consulted: substring scan still finds everything.
    assert_eq!(ids(&store, "北喝咖").await, [1]);
    assert_eq!(ids(&store, "runner").await, [2]);
    assert_eq!(ids(&store, "北").await, [1]);
    assert!(
        store.search(query("北喝咖")).await.unwrap().items[0]
            .snippet
            .is_some()
    );

    let mut reports = Vec::new();
    let done = store
        .rebuild_search_index(1, |progress| reports.push(progress))
        .await
        .unwrap();
    assert_eq!((done.indexed, done.total), (2, 2));
    assert!(reports.len() >= 3, "batched progress: {reports:?}");
    let status = store.search_index_status().await.unwrap();
    assert_eq!(
        (status.state, status.version, status.indexed, status.total),
        (SearchIndexState::Ready, 1, 2, 2)
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM messages_fts_docsize").await,
        2
    );
    assert_eq!(ids(&store, "台北咖啡").await, [1]);
}

#[tokio::test]
async fn rebuilding_state_uses_like_fallback() {
    let (_dir, store, url) = store().await;
    add(&store, vec![message(10, 1, "台北咖啡")]).await;
    let pool = raw(&url).await;
    sqlx::query("UPDATE app_metadata SET value='rebuilding' WHERE key='search_index_state'")
        .execute(&pool)
        .await
        .unwrap();
    let status = store.search_index_status().await.unwrap();
    assert_eq!(status.state, SearchIndexState::Rebuilding);
    assert_eq!(ids(&store, "台北咖啡").await, [1]);
}

#[tokio::test]
async fn query_validation() {
    let (_dir, store, _) = store().await;
    assert!(query("   ").validate().is_err());
    assert!(query("！？ * \"").validate().is_err());
    assert!(query("北").validate().is_ok());
    let mut long = query(&"a".repeat(4097));
    assert!(long.validate().is_err());
    long.text = "a".repeat(4096);
    assert!(long.validate().is_ok());
    let mut keyset_with_relevance = query("a");
    keyset_with_relevance.before = Some(MessageCursor {
        timestamp: DateTime::from_timestamp(1, 0).unwrap(),
        chat_id: chat_id(10),
        message_id: MessageId::new(1).unwrap(),
    });
    assert!(keyset_with_relevance.validate().is_err());
    // Hostile FTS syntax is just text.
    add(
        &store,
        vec![message(10, 1, "alpha OR beta \"quoted\" -x *")],
    )
    .await;
    for text in [
        "alpha\" OR *",
        "NEAR(a b)",
        "col:val",
        "a AND",
        "\"",
        "-x",
        "beta*",
    ] {
        let _ = store.search(query(text)).await;
    }
    assert_eq!(ids(&store, "alpha OR beta").await, [1]);
}

const OLD_SCHEMA_MIGRATIONS: [&str; 5] = [
    "0001_archive.sql",
    "0002_telegram_account_deletions.sql",
    "0003_chat_tracking.sql",
    "0004_sync_rate_limited_state.sql",
    "0005_read_indexes.sql",
];

/// Creates a database exactly as released versions (migrations 0001-0005) left it, with the old
/// external-content FTS table and its triggers maintaining the index.
async fn create_old_database(url: &str) {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let staging = tempfile::tempdir().unwrap();
    for name in OLD_SCHEMA_MIGRATIONS {
        std::fs::copy(source.join(name), staging.path().join(name)).unwrap();
    }
    let options = url
        .parse::<sqlx::sqlite::SqliteConnectOptions>()
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePool::connect_with(options).await.unwrap();
    Migrator::new(staging.path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO chats(id, kind, title, created_at, updated_at) VALUES (10, 'group', 'old', 1, 1)")
        .execute(&pool)
        .await
        .unwrap();
    for (id, text) in [(1, "今天去台北喝咖啡"), (2, "GitLab Runner 沒有回應")] {
        sqlx::query("INSERT INTO messages(chat_id, message_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (10, ?, ?, 1, 1, 1, ?, ?, 0)")
            .bind(id)
            .bind(BASE + id)
            .bind(text)
            .bind(BASE + id)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO messages(chat_id, message_id, timestamp, collected_at, created_at, updated_at, text, version_at, source_priority) VALUES (10, 3, ?, 1, 1, 1, NULL, ?, 0)")
        .bind(BASE + 3)
        .bind(BASE + 3)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'"
        )
        .await,
        3
    );
    pool.close().await;
}

#[tokio::test]
async fn upgrade_from_0005_schema_drops_triggers_and_rebuild_fills_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("old.db").display());
    create_old_database(&url).await;

    let store = SqliteStore::connect(&url).await.unwrap();
    let pool = raw(&url).await;
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger'"
        )
        .await,
        0
    );
    let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='messages_fts'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(sql.contains("contentless_delete=1"), "{sql}");

    // Existing data, empty index: stale, and search still works through the LIKE fallback.
    let status = store.search_index_status().await.unwrap();
    assert_eq!(
        (status.state, status.version, status.indexed, status.total),
        (SearchIndexState::Stale, 0, 0, 2)
    );
    assert!(!store.search_index_ready().await.unwrap());
    assert_eq!(ids(&store, "北喝咖").await, [1]);
    assert_eq!(ids(&store, "gitlab runner").await, [2]);

    // New writes are indexed even before the rebuild, and stay stale until it runs.
    add(&store, vec![message(10, 4, "新的台北訊息")]).await;
    assert_eq!(
        store.search_index_status().await.unwrap().state,
        SearchIndexState::Stale
    );

    store.rebuild_search_index(1000, |_| {}).await.unwrap();
    let status = store.search_index_status().await.unwrap();
    assert_eq!(
        (status.state, status.version, status.indexed, status.total),
        (SearchIndexState::Ready, 1, 3, 3)
    );
    assert_eq!(ids(&store, "台北").await, [1, 4]);
    assert_eq!(ids(&store, "北喝咖").await, [1]);

    // A second connect (e.g. the next `db init`) keeps the ready state.
    store.close().await;
    let reopened = SqliteStore::connect(&url).await.unwrap();
    assert!(reopened.search_index_ready().await.unwrap());
}

#[tokio::test]
async fn read_only_open_of_an_unmigrated_database_reports_stale_and_still_searches() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("old.db").display());
    create_old_database(&url).await;
    let readonly = SqliteStore::open_existing_readonly(&url).await.unwrap();
    let status = readonly.search_index_status().await.unwrap();
    assert_eq!(status.state, SearchIndexState::Stale);
    assert_eq!(ids(&readonly, "台北").await, [1]);
}

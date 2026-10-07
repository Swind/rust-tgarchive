use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, Transaction, sqlite::SqliteRow};

use crate::{
    application::{
        ArchiveWriter, ChatCheckpoint, ChatRepository, ChatSort, ChatStats, ChatSummary,
        IngestBatch, MessageRepository, MessageSource, MessageView, PageSize, RefetchCheckpoint,
        RepositoryError, SenderDetail, SenderInfo, SenderPage, SenderQuery, SenderSummary,
        SyncChatProgress, SyncJob, SyncJobState, SyncRepository, SyncScope, TrackingScope,
    },
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Forward, Message, MessageEvent,
        MessageId, Sender, SenderId, SenderKind,
    },
};

use super::{
    media::enqueue_attachment_downloads,
    open_existing_pool, open_pool, open_readonly_pool,
    search::{index_message, read_state},
    storage_error,
};
use crate::infrastructure::search::{SearchTokenizer, default_tokenizer, make_snippet, plan_query};

#[derive(Clone)]
pub struct SqliteStore {
    pub(super) pool: SqlitePool,
    pub(super) tokenizer: Arc<dyn SearchTokenizer>,
    /// Read-only database opened without migrating: lacks the 0007 columns.
    pub(super) legacy_schema: bool,
    /// Read-only database opened without migrating: lacks `senders.is_bot` and the name history
    /// table (migration 0008). Bot flags read as NULL and histories as empty.
    pub(super) pre_0008: bool,
    /// Read-only database opened without migrating: lacks media tables from migration 0009.
    pub(super) pre_0009_media: bool,
}

/// Which migrations a read-only, unmigrated database is missing.
#[derive(Clone, Copy, Default)]
pub(super) struct LegacyColumns {
    pub(super) metadata: bool,
    pub(super) bot: bool,
}

impl SqliteStore {
    fn with_pool(pool: SqlitePool) -> Self {
        Self {
            pool,
            tokenizer: default_tokenizer(),
            legacy_schema: false,
            pre_0008: false,
            pre_0009_media: false,
        }
    }

    /// Replaces the shared jieba tokenizer (tests).
    pub fn with_tokenizer(mut self, tokenizer: Arc<dyn SearchTokenizer>) -> Self {
        self.tokenizer = tokenizer;
        self
    }

    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        Ok(Self::with_pool(open_pool(database_url).await?))
    }

    /// Like [`Self::connect`] (applies migrations) but fails instead of creating a new database.
    pub async fn open_existing(database_url: &str) -> Result<Self, RepositoryError> {
        Ok(Self::with_pool(open_existing_pool(database_url).await?))
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Binds this single-account archive to a Telegram user and rejects reuse for another user.
    pub async fn bind_telegram_account(&self, user_id: SenderId) -> Result<(), RepositoryError> {
        if user_id.get() <= 0 {
            return Err(invalid_data("Telegram account identity must be a user ID"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT OR IGNORE INTO telegram_account_identity(singleton, user_id, bound_at) VALUES (1, ?, unixepoch())")
            .bind(user_id.get())
            .execute(&mut *tx)
            .await?;
        let saved: i64 =
            sqlx::query_scalar("SELECT user_id FROM telegram_account_identity WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await?;
        if saved != user_id.get() {
            return Err(invalid_data(format!(
                "archive is bound to Telegram user {saved}, not {}",
                user_id.get()
            )));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Number of account-wide deletion IDs that do not resolve to exactly one common-chat row.
    pub async fn unresolved_common_deletion_count(&self) -> Result<u64, RepositoryError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM (SELECT t.message_id FROM common_message_tombstones t LEFT JOIN messages m ON m.message_id=t.message_id AND m.chat_id > -1000000000000 GROUP BY t.message_id HAVING COUNT(m.row_id) != 1)",
        )
        .fetch_one(&self.pool)
        .await?;
        u64::try_from(count).map_err(invalid_data)
    }
}

impl SqliteStore {
    pub(super) fn legacy(&self) -> LegacyColumns {
        LegacyColumns {
            metadata: self.legacy_schema,
            bot: self.pre_0008,
        }
    }

    pub async fn open_existing_readonly(database_url: &str) -> Result<Self, RepositoryError> {
        let mut store = Self::with_pool(open_readonly_pool(database_url).await?);
        let columns: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='post_author'",
        )
        .fetch_one(&store.pool)
        .await?;
        store.legacy_schema = columns == 0;
        let bot: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('senders') WHERE name='is_bot'",
        )
        .fetch_one(&store.pool)
        .await?;
        store.pre_0008 = bot == 0;
        let media: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='media_downloads'",
        )
        .fetch_one(&store.pool)
        .await?;
        store.pre_0009_media = media == 0;
        Ok(store)
    }
}

impl From<sqlx::Error> for RepositoryError {
    fn from(error: sqlx::Error) -> Self {
        storage_error(error)
    }
}

pub(super) fn invalid_data(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::InvalidData(error.to_string())
}

pub(super) fn timestamp(seconds: i64) -> Result<DateTime<Utc>, RepositoryError> {
    DateTime::from_timestamp(seconds, 0).ok_or_else(|| invalid_data("timestamp is out of range"))
}

fn seconds(value: DateTime<Utc>) -> i64 {
    value.timestamp()
}

fn ceil_seconds(value: DateTime<Utc>) -> Result<i64, RepositoryError> {
    let seconds = value.timestamp();
    if value.timestamp_subsec_nanos() == 0 {
        Ok(seconds)
    } else {
        seconds
            .checked_add(1)
            .ok_or_else(|| invalid_data("timestamp is out of range"))
    }
}

fn chat_kind(value: ChatKind) -> &'static str {
    match value {
        ChatKind::Private => "private",
        ChatKind::Group => "group",
        ChatKind::Supergroup => "supergroup",
        ChatKind::Channel => "channel",
    }
}

pub(super) fn parse_chat_kind(value: &str) -> Result<ChatKind, RepositoryError> {
    match value {
        "private" => Ok(ChatKind::Private),
        "group" => Ok(ChatKind::Group),
        "supergroup" => Ok(ChatKind::Supergroup),
        "channel" => Ok(ChatKind::Channel),
        _ => Err(invalid_data(format!("unknown chat kind {value:?}"))),
    }
}

fn sender_kind(value: SenderKind) -> &'static str {
    match value {
        SenderKind::User => "user",
        SenderKind::Chat => "chat",
        SenderKind::Channel => "channel",
        SenderKind::Unknown => "unknown",
    }
}

fn attachment_kind(value: AttachmentKind) -> &'static str {
    match value {
        AttachmentKind::Photo => "photo",
        AttachmentKind::Video => "video",
        AttachmentKind::Audio => "audio",
        AttachmentKind::Voice => "voice",
        AttachmentKind::Document => "document",
        AttachmentKind::Sticker => "sticker",
        AttachmentKind::Animation => "animation",
        AttachmentKind::Other => "other",
    }
}

fn parse_attachment_kind(value: &str) -> Result<AttachmentKind, RepositoryError> {
    match value {
        "photo" => Ok(AttachmentKind::Photo),
        "video" => Ok(AttachmentKind::Video),
        "audio" => Ok(AttachmentKind::Audio),
        "voice" => Ok(AttachmentKind::Voice),
        "document" => Ok(AttachmentKind::Document),
        "sticker" => Ok(AttachmentKind::Sticker),
        "animation" => Ok(AttachmentKind::Animation),
        "other" => Ok(AttachmentKind::Other),
        _ => Err(invalid_data(format!("unknown attachment kind {value:?}"))),
    }
}

async fn ensure_chat(tx: &mut Transaction<'_, Sqlite>, id: ChatId) -> Result<(), RepositoryError> {
    let kind = if id.get() > 0 {
        "private"
    } else if id.get() > -1_000_000_000_000 {
        "group"
    } else {
        "supergroup"
    };
    sqlx::query("INSERT INTO chats(id, kind, created_at, updated_at) VALUES (?, ?, unixepoch(), unixepoch()) ON CONFLICT(id) DO NOTHING")
        .bind(id.get()).bind(kind).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO chat_sync_state(chat_id, updated_at) VALUES (?, unixepoch()) ON CONFLICT(chat_id) DO NOTHING")
        .bind(id.get()).execute(&mut **tx).await?;
    Ok(())
}

async fn save_chat(tx: &mut Transaction<'_, Sqlite>, chat: &Chat) -> Result<(), RepositoryError> {
    sqlx::query("INSERT INTO chats(id, kind, title, username, created_at, updated_at) VALUES (?, ?, ?, ?, unixepoch(), unixepoch()) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, title=COALESCE(excluded.title, chats.title), username=COALESCE(excluded.username, chats.username), updated_at=unixepoch()")
        .bind(chat.id.get()).bind(chat_kind(chat.kind)).bind(&chat.title).bind(&chat.username)
        .execute(&mut **tx).await?;
    sqlx::query("INSERT INTO chat_sync_state(chat_id, updated_at) VALUES (?, unixepoch()) ON CONFLICT(chat_id) DO NOTHING")
        .bind(chat.id.get()).execute(&mut **tx).await?;
    Ok(())
}

fn row_chat(row: &SqliteRow) -> Result<Chat, RepositoryError> {
    Ok(Chat {
        id: ChatId::from_marked(row.try_get("id").map_err(storage_error)?).map_err(invalid_data)?,
        kind: parse_chat_kind(row.try_get("kind").map_err(storage_error)?)?,
        title: row.try_get("title").map_err(storage_error)?,
        username: row.try_get("username").map_err(storage_error)?,
        tracked: row.try_get::<i64, _>("tracked").map_err(storage_error)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_pool_connection_enables_foreign_keys_and_uses_wal() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}", dir.path().join("pool.db").display());
        let store = SqliteStore::connect(&url).await.unwrap();
        let mut connections = Vec::new();
        for _ in 0..5 {
            connections.push(store.pool.acquire().await.unwrap());
        }
        for connection in &mut connections {
            let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            assert_eq!(foreign_keys, 1);
            assert_eq!(journal_mode.to_lowercase(), "wal");
            assert_eq!(synchronous, 1);
        }
        drop(connections);
        let plan = sqlx::query("EXPLAIN QUERY PLAN SELECT chat_id, message_id FROM messages WHERE is_deleted=0 ORDER BY timestamp DESC, chat_id DESC, message_id DESC LIMIT 10")
            .fetch_all(&store.pool).await.unwrap();
        let details = plan
            .iter()
            .map(|row| row.try_get::<String, _>("detail").unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            details.contains("messages_global_order"),
            "unexpected plan: {details}"
        );
        store.close().await;
    }
}

mod archive;
mod chats;
mod messages;
mod sync;
mod tracking;

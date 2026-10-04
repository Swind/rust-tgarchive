use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool, Transaction, sqlite::SqliteRow};

use crate::{
    application::{
        ArchiveWriter, ChatCheckpoint, ChatRepository, ChatSort, ChatStats, ChatSummary,
        IngestBatch, MessageRepository, MessageView, PageSize, RepositoryError, SenderInfo,
        SenderSummary, SyncChatProgress, SyncJob, SyncJobState, SyncRepository, SyncScope,
        TrackingScope,
    },
    domain::{
        Attachment, AttachmentKind, Chat, ChatId, ChatKind, Message, MessageEvent, MessageId,
        Sender, SenderId, SenderKind,
    },
};

use super::{open_existing_pool, open_pool, open_readonly_pool, storage_error};

#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub async fn connect(database_url: &str) -> Result<Self, RepositoryError> {
        Ok(Self {
            pool: open_pool(database_url).await?,
        })
    }

    /// Like [`Self::connect`] (applies migrations) but fails instead of creating a new database.
    pub async fn open_existing(database_url: &str) -> Result<Self, RepositoryError> {
        Ok(Self {
            pool: open_existing_pool(database_url).await?,
        })
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
    pub async fn open_existing_readonly(database_url: &str) -> Result<Self, RepositoryError> {
        Ok(Self {
            pool: open_readonly_pool(database_url).await?,
        })
    }
}

impl From<sqlx::Error> for RepositoryError {
    fn from(error: sqlx::Error) -> Self {
        storage_error(error)
    }
}

fn invalid_data(error: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::InvalidData(error.to_string())
}

fn timestamp(seconds: i64) -> Result<DateTime<Utc>, RepositoryError> {
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

fn parse_chat_kind(value: &str) -> Result<ChatKind, RepositoryError> {
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

async fn save_sender(
    tx: &mut Transaction<'_, Sqlite>,
    sender: &Sender,
) -> Result<(), RepositoryError> {
    sqlx::query("INSERT INTO senders(id, kind, display_name, username, created_at, updated_at) VALUES (?, ?, ?, ?, unixepoch(), unixepoch()) ON CONFLICT(id) DO UPDATE SET kind=CASE WHEN excluded.kind='unknown' THEN senders.kind ELSE excluded.kind END, display_name=COALESCE(excluded.display_name, senders.display_name), username=COALESCE(excluded.username, senders.username), updated_at=unixepoch()")
        .bind(sender.id.get()).bind(sender_kind(sender.kind)).bind(&sender.display_name).bind(&sender.username)
        .execute(&mut **tx).await?;
    Ok(())
}

async fn replace_attachments(
    tx: &mut Transaction<'_, Sqlite>,
    row_id: i64,
    attachments: &[Attachment],
) -> Result<(), RepositoryError> {
    sqlx::query("DELETE FROM attachments WHERE message_row_id=?")
        .bind(row_id)
        .execute(&mut **tx)
        .await?;
    for (ordinal, attachment) in attachments.iter().enumerate() {
        sqlx::query("INSERT INTO attachments(message_row_id, ordinal, kind, telegram_file_id, mime_type, file_name, size) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(row_id).bind(ordinal as i64).bind(attachment_kind(attachment.kind))
            .bind(&attachment.telegram_file_id).bind(&attachment.mime_type).bind(&attachment.file_name).bind(attachment.size)
            .execute(&mut **tx).await?;
    }
    Ok(())
}

fn transition_allowed(from: &SyncJobState, to: &SyncJobState) -> bool {
    from == to
        || matches!(
            (from, to),
            (
                SyncJobState::Queued,
                SyncJobState::Running | SyncJobState::Failed | SyncJobState::Interrupted
            ) | (
                SyncJobState::Running,
                SyncJobState::Succeeded
                    | SyncJobState::Failed
                    | SyncJobState::Interrupted
                    | SyncJobState::RateLimited
            )
        )
}

async fn save_progress(
    tx: &mut Transaction<'_, Sqlite>,
    progress: SyncChatProgress,
) -> Result<(), RepositoryError> {
    let count = i64::try_from(progress.committed_messages).map_err(invalid_data)?;
    let old = sqlx::query(
        "SELECT state, committed_count FROM sync_job_chats WHERE job_id=? AND chat_id=?",
    )
    .bind(&progress.job_id)
    .bind(progress.chat_id.get())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = old {
        let state = parse_job_state(row.try_get("state").map_err(storage_error)?)?;
        let old_count: i64 = row.try_get("committed_count").map_err(storage_error)?;
        if !transition_allowed(&state, &progress.state) || count < old_count {
            return Err(invalid_data("sync chat progress cannot move backwards"));
        }
    }
    sqlx::query("INSERT INTO sync_job_chats(job_id, chat_id, state, committed_count, error_summary) VALUES (?, ?, ?, ?, ?) ON CONFLICT(job_id, chat_id) DO UPDATE SET state=excluded.state, committed_count=excluded.committed_count, error_summary=excluded.error_summary")
        .bind(progress.job_id).bind(progress.chat_id.get()).bind(job_state(progress.state)).bind(count).bind(progress.summary_error)
        .execute(&mut **tx).await?;
    Ok(())
}

#[async_trait::async_trait]
impl ArchiveWriter for SqliteStore {
    async fn write_batch(&self, batch: IngestBatch) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        if !batch.account_deletions.is_empty() {
            let bound: Option<i64> = sqlx::query_scalar(
                "SELECT user_id FROM telegram_account_identity WHERE singleton=1",
            )
            .fetch_optional(&mut *tx)
            .await?;
            if bound.is_none() {
                return Err(invalid_data(
                    "bind the Telegram account before writing account-wide deletions",
                ));
            }
            for deletion in &batch.account_deletions {
                sqlx::query("INSERT INTO common_message_tombstones(message_id, deleted_at) VALUES (?, ?) ON CONFLICT(message_id) DO UPDATE SET deleted_at=MAX(common_message_tombstones.deleted_at, excluded.deleted_at)")
                    .bind(deletion.message_id.get())
                    .bind(seconds(deletion.deleted_at))
                    .execute(&mut *tx)
                    .await?;
                let matches: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM messages WHERE message_id=? AND chat_id > -1000000000000",
                )
                .bind(deletion.message_id.get())
                .fetch_one(&mut *tx)
                .await?;
                if matches == 1 {
                    sqlx::query("UPDATE messages SET is_deleted=1, deleted_at=MAX(COALESCE(deleted_at, 0), ?), updated_at=unixepoch() WHERE message_id=? AND chat_id > -1000000000000")
                        .bind(seconds(deletion.deleted_at))
                        .bind(deletion.message_id.get())
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
        for chat in &batch.chats {
            save_chat(&mut tx, chat).await?;
        }
        for sender in &batch.senders {
            save_sender(&mut tx, sender).await?;
        }

        for record in &batch.records {
            match &record.event {
                MessageEvent::Deleted {
                    chat_id,
                    message_id,
                    deleted_at,
                } => {
                    ensure_chat(&mut tx, *chat_id).await?;
                    sqlx::query("INSERT INTO message_tombstones(chat_id, message_id, deleted_at) VALUES (?, ?, ?) ON CONFLICT(chat_id, message_id) DO UPDATE SET deleted_at=MAX(message_tombstones.deleted_at, excluded.deleted_at)")
                        .bind(chat_id.get()).bind(message_id.get()).bind(seconds(*deleted_at))
                        .execute(&mut *tx).await?;
                    sqlx::query("UPDATE messages SET is_deleted=1, deleted_at=MAX(COALESCE(deleted_at, 0), ?), updated_at=unixepoch() WHERE chat_id=? AND message_id=?")
                        .bind(seconds(*deleted_at)).bind(chat_id.get()).bind(message_id.get())
                        .execute(&mut *tx).await?;
                }
                MessageEvent::Created(message) | MessageEvent::Updated(message) => {
                    ensure_chat(&mut tx, message.chat_id).await?;
                    let version = seconds(message.edited_at.unwrap_or(message.timestamp));
                    let priority =
                        if matches!(record.source, crate::application::MessageSource::Realtime) {
                            1i64
                        } else {
                            0
                        };
                    let result = sqlx::query("INSERT INTO messages(chat_id, message_id, sender_id, timestamp, edited_at, collected_at, created_at, updated_at, text, reply_to, version_at, source_priority) SELECT ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch(), ?, ?, ?, ? WHERE NOT EXISTS(SELECT 1 FROM message_tombstones WHERE chat_id=? AND message_id=?) AND NOT (? > -1000000000000 AND EXISTS(SELECT 1 FROM common_message_tombstones WHERE message_id=?)) ON CONFLICT(chat_id, message_id) DO UPDATE SET sender_id=excluded.sender_id, timestamp=excluded.timestamp, edited_at=excluded.edited_at, collected_at=excluded.collected_at, updated_at=unixepoch(), text=excluded.text, reply_to=excluded.reply_to, version_at=excluded.version_at, source_priority=excluded.source_priority WHERE (excluded.version_at > messages.version_at OR (excluded.version_at = messages.version_at AND excluded.source_priority >= messages.source_priority)) AND messages.is_deleted=0")
                        .bind(message.chat_id.get()).bind(message.id.get()).bind(message.sender_id.map(SenderId::get))
                        .bind(seconds(message.timestamp)).bind(message.edited_at.map(seconds)).bind(seconds(message.collected_at))
                        .bind(&message.text).bind(message.reply_to.map(MessageId::get)).bind(version).bind(priority)
                        .bind(message.chat_id.get()).bind(message.id.get())
                        .bind(message.chat_id.get()).bind(message.id.get())
                        .execute(&mut *tx).await?;
                    if result.rows_affected() > 0 {
                        let row_id: i64 = sqlx::query_scalar(
                            "SELECT row_id FROM messages WHERE chat_id=? AND message_id=?",
                        )
                        .bind(message.chat_id.get())
                        .bind(message.id.get())
                        .fetch_one(&mut *tx)
                        .await?;
                        replace_attachments(&mut tx, row_id, &message.attachments).await?;
                        sqlx::query("UPDATE chat_sync_state SET oldest_message_id=CASE WHEN oldest_message_id IS NULL OR ?<oldest_message_id THEN ? ELSE oldest_message_id END, newest_message_id=CASE WHEN newest_message_id IS NULL OR ?>newest_message_id THEN ? ELSE newest_message_id END, updated_at=unixepoch() WHERE chat_id=?")
                            .bind(message.id.get()).bind(message.id.get()).bind(message.id.get()).bind(message.id.get()).bind(message.chat_id.get())
                            .execute(&mut *tx).await?;
                    }
                }
            }
        }

        if let Some((chat_id, checkpoint)) = batch.checkpoint {
            ensure_chat(&mut tx, chat_id).await?;
            sqlx::query("INSERT INTO chat_sync_state(chat_id, history_before_id, history_complete, catchup_after_id, updated_at) VALUES (?, ?, ?, ?, unixepoch()) ON CONFLICT(chat_id) DO UPDATE SET history_before_id=CASE WHEN chat_sync_state.history_before_id IS NULL THEN excluded.history_before_id WHEN excluded.history_before_id IS NULL THEN chat_sync_state.history_before_id ELSE MIN(chat_sync_state.history_before_id, excluded.history_before_id) END, history_complete=MAX(chat_sync_state.history_complete, excluded.history_complete), catchup_after_id=CASE WHEN chat_sync_state.catchup_after_id IS NULL THEN excluded.catchup_after_id WHEN excluded.catchup_after_id IS NULL THEN chat_sync_state.catchup_after_id ELSE MAX(chat_sync_state.catchup_after_id, excluded.catchup_after_id) END, last_error=NULL, updated_at=unixepoch()")
                .bind(chat_id.get()).bind(checkpoint.history_before_id.map(MessageId::get))
                .bind(checkpoint.history_complete).bind(checkpoint.catchup_after_id.map(MessageId::get))
                .execute(&mut *tx).await?;
        }
        if let Some((chat_id, reason)) = batch.chat_error {
            ensure_chat(&mut tx, chat_id).await?;
            sqlx::query(
                "UPDATE chat_sync_state SET last_error=?, updated_at=unixepoch() WHERE chat_id=?",
            )
            .bind(reason)
            .bind(chat_id.get())
            .execute(&mut *tx)
            .await?;
        }
        if let Some(progress) = batch.job_progress {
            ensure_chat(&mut tx, progress.chat_id).await?;
            save_progress(&mut tx, progress).await?;
        }
        tx.commit().await.map_err(storage_error)
    }
}

fn job_state(state: SyncJobState) -> &'static str {
    match state {
        SyncJobState::Queued => "queued",
        SyncJobState::Running => "running",
        SyncJobState::Succeeded => "succeeded",
        SyncJobState::Failed => "failed",
        SyncJobState::Interrupted => "interrupted",
        SyncJobState::RateLimited => "rate_limited",
    }
}

fn parse_job_state(value: &str) -> Result<SyncJobState, RepositoryError> {
    match value {
        "queued" => Ok(SyncJobState::Queued),
        "running" => Ok(SyncJobState::Running),
        "succeeded" => Ok(SyncJobState::Succeeded),
        "failed" => Ok(SyncJobState::Failed),
        "interrupted" => Ok(SyncJobState::Interrupted),
        "rate_limited" => Ok(SyncJobState::RateLimited),
        _ => Err(invalid_data(format!("unknown sync job state {value:?}"))),
    }
}

fn parse_scope(scope: &str, chat_id: Option<i64>) -> Result<SyncScope, RepositoryError> {
    match (scope, chat_id) {
        ("all", None) => Ok(SyncScope::All),
        ("chat", Some(id)) => Ok(SyncScope::Chat(
            ChatId::from_marked(id).map_err(invalid_data)?,
        )),
        _ => Err(invalid_data("invalid sync job scope")),
    }
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

fn literal_fts_query(text: &str) -> Result<String, RepositoryError> {
    if text.len() > 512 {
        return Err(invalid_data("search query exceeds 512 bytes"));
    }
    let terms: Vec<_> = text.split_whitespace().collect();
    if terms.is_empty() {
        return Err(invalid_data("search query cannot be empty"));
    }
    Ok(terms
        .into_iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}

async fn attachments_for(
    tx: &mut Transaction<'_, Sqlite>,
    row_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<Attachment>>, RepositoryError> {
    let mut attachments = std::collections::HashMap::<i64, Vec<Attachment>>::new();
    if row_ids.is_empty() {
        return Ok(attachments);
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT message_row_id, kind, telegram_file_id, mime_type, file_name, size FROM attachments WHERE message_row_id IN (",
    );
    let mut separated = query.separated(", ");
    for id in row_ids {
        separated.push_bind(id);
    }
    separated.push_unseparated(") ORDER BY message_row_id, ordinal");
    for row in query.build().fetch_all(&mut **tx).await? {
        let row_id: i64 = row.try_get("message_row_id").map_err(storage_error)?;
        attachments.entry(row_id).or_default().push(Attachment {
            kind: parse_attachment_kind(row.try_get("kind").map_err(storage_error)?)?,
            telegram_file_id: row.try_get("telegram_file_id").map_err(storage_error)?,
            mime_type: row.try_get("mime_type").map_err(storage_error)?,
            file_name: row.try_get("file_name").map_err(storage_error)?,
            size: row.try_get("size").map_err(storage_error)?,
        });
    }
    Ok(attachments)
}

const MESSAGE_SELECT: &str = "SELECT m.row_id, m.chat_id, m.message_id, m.sender_id, m.timestamp, m.edited_at, m.collected_at, m.text, m.reply_to, CASE WHEN m.is_deleted=1 THEN COALESCE(m.deleted_at, 0) END AS deleted_at, s.display_name AS sender_name, s.username AS sender_username, c.title AS chat_title FROM messages m LEFT JOIN senders s ON s.id=m.sender_id LEFT JOIN chats c ON c.id=m.chat_id";

fn row_message(row: &SqliteRow, attachments: Vec<Attachment>) -> Result<Message, RepositoryError> {
    let sender: Option<i64> = row.try_get("sender_id").map_err(storage_error)?;
    let edited: Option<i64> = row.try_get("edited_at").map_err(storage_error)?;
    let reply: Option<i64> = row.try_get("reply_to").map_err(storage_error)?;
    Ok(Message {
        id: MessageId::new(row.try_get("message_id").map_err(storage_error)?)
            .map_err(invalid_data)?,
        chat_id: ChatId::from_marked(row.try_get("chat_id").map_err(storage_error)?)
            .map_err(invalid_data)?,
        sender_id: sender
            .map(SenderId::from_marked)
            .transpose()
            .map_err(invalid_data)?,
        timestamp: timestamp(row.try_get("timestamp").map_err(storage_error)?)?,
        edited_at: edited.map(timestamp).transpose()?,
        collected_at: timestamp(row.try_get("collected_at").map_err(storage_error)?)?,
        text: row.try_get("text").map_err(storage_error)?,
        reply_to: reply
            .map(MessageId::new)
            .transpose()
            .map_err(invalid_data)?,
        attachments,
    })
}

fn row_view(row: &SqliteRow, attachments: Vec<Attachment>) -> Result<MessageView, RepositoryError> {
    let message = row_message(row, attachments)?;
    let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(storage_error)?;
    let name: Option<String> = row.try_get("sender_name").map_err(storage_error)?;
    let username: Option<String> = row.try_get("sender_username").map_err(storage_error)?;
    let chat_title: Option<String> = row.try_get("chat_title").map_err(storage_error)?;
    let sender = message.sender_id.map(|id| SenderInfo {
        id,
        // A chat/channel posting as itself has no profile name of its own: use the chat title.
        display_name: name.or_else(|| {
            (id.get() == message.chat_id.get())
                .then_some(chat_title)
                .flatten()
        }),
        username,
    });
    Ok(MessageView {
        message,
        sender,
        deleted_at: deleted_at.map(timestamp).transpose()?,
    })
}

async fn load_messages(
    tx: &mut Transaction<'_, Sqlite>,
    rows: Vec<SqliteRow>,
) -> Result<Vec<MessageView>, RepositoryError> {
    let row_ids = rows
        .iter()
        .map(|row| row.try_get("row_id").map_err(storage_error))
        .collect::<Result<Vec<i64>, _>>()?;
    let attachments = attachments_for(tx, &row_ids).await?;
    rows.iter()
        .zip(row_ids)
        .map(|(row, row_id)| row_view(row, attachments.get(&row_id).cloned().unwrap_or_default()))
        .collect()
}

async fn list_messages(
    tx: &mut Transaction<'_, Sqlite>,
    filters: &crate::application::MessageFilters,
    before: Option<&crate::application::MessageCursor>,
    after: Option<&crate::application::MessageCursor>,
    page_size: crate::application::PageSize,
    search: Option<&str>,
) -> Result<crate::application::MessagePage, RepositoryError> {
    let after_direction = after.is_some();
    let mut query = QueryBuilder::<Sqlite>::new(MESSAGE_SELECT);
    if search.is_some() {
        query.push(" JOIN messages_fts ON messages_fts.rowid=m.row_id");
    }
    query.push(if filters.include_deleted {
        " WHERE 1=1"
    } else {
        " WHERE m.is_deleted=0"
    });
    if let Some(chat_id) = filters.chat_id {
        query.push(" AND m.chat_id=").push_bind(chat_id.get());
    }
    if let Some(sender_id) = filters.sender_id {
        query.push(" AND m.sender_id=").push_bind(sender_id.get());
    }
    if let Some(from) = filters.time_range.from {
        query
            .push(" AND m.timestamp>=")
            .push_bind(ceil_seconds(from)?);
    }
    if let Some(to) = filters.time_range.to {
        query.push(" AND m.timestamp<").push_bind(ceil_seconds(to)?);
    }
    if let Some(text) = search {
        query
            .push(" AND messages_fts MATCH ")
            .push_bind(literal_fts_query(text)?);
    }
    if let Some(cursor) = before.or(after) {
        if cursor.timestamp.timestamp_subsec_nanos() == 0 {
            query.push(if after_direction {
                " AND (m.timestamp, m.chat_id, m.message_id)>("
            } else {
                " AND (m.timestamp, m.chat_id, m.message_id)<("
            });
            query
                .push_bind(seconds(cursor.timestamp))
                .push(", ")
                .push_bind(cursor.chat_id.get())
                .push(", ")
                .push_bind(cursor.message_id.get())
                .push(")");
        } else if after_direction {
            query
                .push(" AND m.timestamp>")
                .push_bind(seconds(cursor.timestamp));
        } else {
            query
                .push(" AND m.timestamp<=")
                .push_bind(seconds(cursor.timestamp));
        }
    }
    query.push(if after_direction {
        " ORDER BY m.timestamp ASC, m.chat_id ASC, m.message_id ASC LIMIT "
    } else {
        " ORDER BY m.timestamp DESC, m.chat_id DESC, m.message_id DESC LIMIT "
    });
    query.push_bind(i64::from(page_size.get()) + 1);
    let mut rows = query.build().fetch_all(&mut **tx).await?;
    let has_more = rows.len() > usize::from(page_size.get());
    rows.truncate(usize::from(page_size.get()));
    if after_direction {
        rows.reverse();
    }
    let items = load_messages(tx, rows).await?;
    let next_cursor = if has_more {
        let item = if after_direction {
            items.first()
        } else {
            items.last()
        };
        item.map(|message| crate::application::MessageCursor {
            timestamp: message.timestamp,
            chat_id: message.chat_id,
            message_id: message.id,
        })
    } else {
        None
    };
    Ok(crate::application::MessagePage {
        items,
        has_more,
        next_cursor,
    })
}

#[async_trait::async_trait]
impl MessageRepository for SqliteStore {
    async fn get(
        &self,
        chat_id: ChatId,
        message_id: MessageId,
        include_deleted: bool,
    ) -> Result<Option<MessageView>, RepositoryError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(&format!(
            "{MESSAGE_SELECT} WHERE m.chat_id=? AND m.message_id=? AND (m.is_deleted=0 OR ?)"
        ))
        .bind(chat_id.get())
        .bind(message_id.get())
        .bind(include_deleted)
        .fetch_optional(&mut *tx)
        .await?;
        let message = match row {
            Some(row) => load_messages(&mut tx, vec![row]).await?.pop(),
            None => None,
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(message)
    }

    async fn list(
        &self,
        query: crate::application::ListMessagesQuery,
    ) -> Result<crate::application::MessagePage, RepositoryError> {
        query.validate().map_err(invalid_data)?;
        let mut tx = self.pool.begin().await?;
        let page = list_messages(
            &mut tx,
            &query.filters,
            query.before.as_ref(),
            query.after.as_ref(),
            query.page_size,
            None,
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(page)
    }

    async fn search(
        &self,
        query: crate::application::SearchMessagesQuery,
    ) -> Result<crate::application::MessagePage, RepositoryError> {
        query.validate().map_err(invalid_data)?;
        let mut tx = self.pool.begin().await?;
        let page = list_messages(
            &mut tx,
            &query.filters,
            query.before.as_ref(),
            query.after.as_ref(),
            query.page_size,
            Some(&query.text),
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(page)
    }

    async fn list_senders(
        &self,
        chat_id: ChatId,
        limit: PageSize,
    ) -> Result<Vec<SenderSummary>, RepositoryError> {
        let rows = sqlx::query("SELECT m.sender_id, COALESCE(s.display_name, CASE WHEN m.sender_id=m.chat_id THEN c.title END) AS display_name, s.username, COUNT(*) AS message_count FROM messages m LEFT JOIN senders s ON s.id=m.sender_id LEFT JOIN chats c ON c.id=m.chat_id WHERE m.chat_id=? AND m.is_deleted=0 AND m.sender_id IS NOT NULL GROUP BY m.sender_id ORDER BY message_count DESC, m.sender_id LIMIT ?")
            .bind(chat_id.get())
            .bind(i64::from(limit.get()))
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                let count: i64 = row.try_get("message_count").map_err(storage_error)?;
                Ok(SenderSummary {
                    sender: SenderInfo {
                        id: SenderId::from_marked(row.try_get("sender_id").map_err(storage_error)?)
                            .map_err(invalid_data)?,
                        display_name: row.try_get("display_name").map_err(storage_error)?,
                        username: row.try_get("username").map_err(storage_error)?,
                    },
                    message_count: u64::try_from(count).map_err(invalid_data)?,
                })
            })
            .collect()
    }
}

fn chat_summary_query(single: bool, sort: ChatSort) -> String {
    let filter = if single { " WHERE chat_id=?" } else { "" };
    let order = match sort {
        ChatSort::Default => "c.id",
        ChatSort::Title => "c.title IS NULL, LOWER(c.title), c.id",
        ChatSort::LastMessage => "a.last_ts IS NULL, a.last_ts DESC, c.id",
        ChatSort::MessageCount => "COALESCE(a.message_count, 0) DESC, c.id",
    };
    format!(
        "SELECT c.id, c.kind, c.title, c.username, c.tracked, COALESCE(a.message_count, 0) AS message_count, COALESCE(a.deleted_count, 0) AS deleted_count, a.first_ts, a.last_ts, COALESCE(st.history_complete, 0) AS history_complete, st.last_sync_completed_at, st.last_error FROM chats c LEFT JOIN (SELECT chat_id, SUM(is_deleted=0) AS message_count, SUM(is_deleted) AS deleted_count, MIN(CASE WHEN is_deleted=0 THEN timestamp END) AS first_ts, MAX(CASE WHEN is_deleted=0 THEN timestamp END) AS last_ts FROM messages{filter} GROUP BY chat_id) a ON a.chat_id=c.id LEFT JOIN chat_sync_state st ON st.chat_id=c.id{} ORDER BY {order}",
        if single { " WHERE c.id=?" } else { "" }
    )
}

fn row_summary(row: &SqliteRow) -> Result<ChatSummary, RepositoryError> {
    let opt_time = |name: &str| -> Result<Option<DateTime<Utc>>, RepositoryError> {
        row.try_get::<Option<i64>, _>(name)
            .map_err(storage_error)?
            .map(timestamp)
            .transpose()
    };
    let count = |name: &str| -> Result<u64, RepositoryError> {
        u64::try_from(row.try_get::<i64, _>(name).map_err(storage_error)?).map_err(invalid_data)
    };
    Ok(ChatSummary {
        chat: row_chat(row)?,
        stats: ChatStats {
            message_count: count("message_count")?,
            deleted_count: count("deleted_count")?,
            first_message_at: opt_time("first_ts")?,
            last_message_at: opt_time("last_ts")?,
            history_complete: row
                .try_get::<i64, _>("history_complete")
                .map_err(storage_error)?
                != 0,
            last_sync_completed_at: opt_time("last_sync_completed_at")?,
            last_error: row.try_get("last_error").map_err(storage_error)?,
        },
    })
}

#[async_trait::async_trait]
impl ChatRepository for SqliteStore {
    async fn list_with_stats(&self, sort: ChatSort) -> Result<Vec<ChatSummary>, RepositoryError> {
        sqlx::query(&chat_summary_query(false, sort))
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_summary)
            .collect()
    }

    async fn get_with_stats(&self, id: ChatId) -> Result<Option<ChatSummary>, RepositoryError> {
        sqlx::query(&chat_summary_query(true, ChatSort::Default))
            .bind(id.get())
            .bind(id.get())
            .fetch_optional(&self.pool)
            .await?
            .as_ref()
            .map(row_summary)
            .transpose()
    }

    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError> {
        let row = sqlx::query("SELECT id, kind, title, username, tracked FROM chats WHERE id=?")
            .bind(id.get())
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_chat).transpose()
    }

    async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
        sqlx::query("SELECT id, kind, title, username, tracked FROM chats ORDER BY id")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(row_chat)
            .collect()
    }

    async fn save_refresh(&self, chats: Vec<Chat>) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        for chat in &chats {
            save_chat(&mut tx, chat).await?;
        }
        tx.commit().await.map_err(storage_error)
    }

    async fn set_tracked(
        &self,
        id: ChatId,
        tracked: bool,
    ) -> Result<Option<Chat>, RepositoryError> {
        sqlx::query("UPDATE chats SET tracked=?1, tracked_at=CASE WHEN ?1=1 THEN COALESCE(tracked_at, unixepoch()) ELSE NULL END WHERE id=?2 AND tracked != ?1")
            .bind(i64::from(tracked))
            .bind(id.get())
            .execute(&self.pool)
            .await?;
        ChatRepository::get(self, id).await
    }
}

#[async_trait::async_trait]
impl TrackingScope for SqliteStore {
    async fn tracked_chat_ids(&self) -> Result<std::collections::HashSet<ChatId>, RepositoryError> {
        sqlx::query_scalar::<_, i64>("SELECT id FROM chats WHERE tracked=1")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|id| ChatId::from_marked(id).map_err(invalid_data))
            .collect()
    }

    async fn archived_common_message_ids(
        &self,
        ids: &[MessageId],
    ) -> Result<std::collections::HashSet<MessageId>, RepositoryError> {
        let mut found = std::collections::HashSet::new();
        for chunk in ids.chunks(500) {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT DISTINCT m.message_id FROM messages m JOIN chats c ON c.id=m.chat_id WHERE c.tracked=1 AND m.chat_id > -1000000000000 AND m.message_id IN (",
            );
            let mut separated = query.separated(", ");
            for id in chunk {
                separated.push_bind(id.get());
            }
            separated.push_unseparated(")");
            for row in query.build().fetch_all(&self.pool).await? {
                let id: i64 = row.try_get("message_id").map_err(storage_error)?;
                found.insert(MessageId::new(id).map_err(invalid_data)?);
            }
        }
        Ok(found)
    }
}

fn row_job(row: &SqliteRow) -> Result<SyncJob, RepositoryError> {
    let scope: String = row.try_get("scope").map_err(storage_error)?;
    let chat_id: Option<i64> = row.try_get("chat_id").map_err(storage_error)?;
    let started_at: Option<i64> = row.try_get("started_at").map_err(storage_error)?;
    let completed_at: Option<i64> = row.try_get("finished_at").map_err(storage_error)?;
    Ok(SyncJob {
        id: row.try_get("id").map_err(storage_error)?,
        scope: parse_scope(&scope, chat_id)?,
        state: parse_job_state(row.try_get("state").map_err(storage_error)?)?,
        created_at: timestamp(row.try_get("created_at").map_err(storage_error)?)?,
        started_at: started_at.map(timestamp).transpose()?,
        completed_at: completed_at.map(timestamp).transpose()?,
        summary_error: row.try_get("error_summary").map_err(storage_error)?,
    })
}

#[async_trait::async_trait]
impl SyncRepository for SqliteStore {
    async fn newest_archived_id(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<MessageId>, RepositoryError> {
        let newest: Option<i64> =
            sqlx::query_scalar("SELECT MAX(message_id) FROM messages WHERE chat_id=?")
                .bind(chat_id.get())
                .fetch_one(&self.pool)
                .await?;
        newest.map(MessageId::new).transpose().map_err(invalid_data)
    }
    async fn get_checkpoint(
        &self,
        chat_id: ChatId,
    ) -> Result<Option<ChatCheckpoint>, RepositoryError> {
        let row = sqlx::query("SELECT history_before_id, history_complete, catchup_after_id FROM chat_sync_state WHERE chat_id=?")
            .bind(chat_id.get()).fetch_optional(&self.pool).await?;
        row.map(|row| {
            let history_before: Option<i64> =
                row.try_get("history_before_id").map_err(storage_error)?;
            let catchup_after: Option<i64> =
                row.try_get("catchup_after_id").map_err(storage_error)?;
            Ok(ChatCheckpoint {
                history_before_id: history_before
                    .map(MessageId::new)
                    .transpose()
                    .map_err(invalid_data)?,
                history_complete: row.try_get("history_complete").map_err(storage_error)?,
                catchup_after_id: catchup_after
                    .map(|id| {
                        if id == 0 {
                            Ok(MessageId::BEFORE_FIRST)
                        } else {
                            MessageId::new(id)
                        }
                    })
                    .transpose()
                    .map_err(invalid_data)?,
            })
        })
        .transpose()
    }

    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError> {
        let (scope, chat_id) = match &job.scope {
            SyncScope::Chat(id) => ("chat", Some(id.get())),
            SyncScope::All => ("all", None),
        };
        let mut tx = self.pool.begin().await?;
        if let Some(old) = sqlx::query("SELECT scope, chat_id, state FROM sync_jobs WHERE id=?")
            .bind(&job.id)
            .fetch_optional(&mut *tx)
            .await?
        {
            let old_scope: String = old.try_get("scope").map_err(storage_error)?;
            let old_chat: Option<i64> = old.try_get("chat_id").map_err(storage_error)?;
            let old_state = parse_job_state(old.try_get("state").map_err(storage_error)?)?;
            if parse_scope(&old_scope, old_chat)? != job.scope
                || !transition_allowed(&old_state, &job.state)
            {
                return Err(invalid_data(
                    "sync job scope or state transition is invalid",
                ));
            }
        }
        sqlx::query("INSERT INTO sync_jobs(id, scope, chat_id, state, created_at, started_at, finished_at, error_summary) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET scope=excluded.scope, chat_id=excluded.chat_id, state=excluded.state, created_at=excluded.created_at, started_at=excluded.started_at, finished_at=excluded.finished_at, error_summary=excluded.error_summary")
            .bind(job.id).bind(scope).bind(chat_id).bind(job_state(job.state)).bind(seconds(job.created_at))
            .bind(job.started_at.map(seconds)).bind(job.completed_at.map(seconds)).bind(job.summary_error)
            .execute(&mut *tx).await?;
        tx.commit().await.map_err(storage_error)
    }

    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError> {
        let row = sqlx::query("SELECT id, scope, chat_id, state, created_at, started_at, finished_at, error_summary FROM sync_jobs WHERE id=?")
            .bind(id).fetch_optional(&self.pool).await?;
        row.as_ref().map(row_job).transpose()
    }

    async fn unresolved_deletions(&self) -> Result<u64, RepositoryError> {
        self.unresolved_common_deletion_count().await
    }

    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        sqlx::query("SELECT id, scope, chat_id, state, created_at, started_at, finished_at, error_summary FROM sync_jobs ORDER BY created_at DESC, id DESC")
            .fetch_all(&self.pool).await?.iter().map(row_job).collect()
    }

    async fn list_chat_progress(
        &self,
        job_id: &str,
    ) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        let rows = sqlx::query("SELECT job_id, chat_id, state, committed_count, error_summary FROM sync_job_chats WHERE job_id=? ORDER BY chat_id")
            .bind(job_id).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let count: i64 = row.try_get("committed_count").map_err(storage_error)?;
                Ok(SyncChatProgress {
                    job_id: row.try_get("job_id").map_err(storage_error)?,
                    chat_id: ChatId::from_marked(row.try_get("chat_id").map_err(storage_error)?)
                        .map_err(invalid_data)?,
                    state: parse_job_state(row.try_get("state").map_err(storage_error)?)?,
                    committed_messages: u64::try_from(count).map_err(invalid_data)?,
                    summary_error: row.try_get("error_summary").map_err(storage_error)?,
                })
            })
            .collect()
    }

    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE sync_jobs SET state='interrupted', finished_at=unixepoch(), error_summary=COALESCE(error_summary, 'process stopped before job completed') WHERE state IN ('queued', 'running')")
            .execute(&mut *tx).await?;
        sqlx::query("UPDATE sync_job_chats SET state='interrupted', error_summary=COALESCE(error_summary, 'process stopped before chat sync completed') WHERE state IN ('queued', 'running')")
            .execute(&mut *tx).await?;
        tx.commit().await.map_err(storage_error)
    }
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

use serde::Serialize;
use sqlx::Row;

use crate::application::RepositoryError;

use super::{storage_error, store::SqliteStore};

/// Rows handled for one chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairChat {
    pub chat_id: i64,
    pub title: Option<String>,
    pub kind: String,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SenderRepairReport {
    pub dry_run: bool,
    /// Rule `channel_post`: NULL-sender rows of `channel` chats get the channel as sender
    /// (rows that were, or with `dry_run` would be, updated).
    pub channel_posts: Vec<RepairChat>,
    pub channel_posts_total: u64,
    /// NULL-sender rows left after the rules ran. Private chats cannot tell incoming from
    /// outgoing without data and are never guessed: re-fetch them with
    /// `tgarchive sync chat <ID> --refetch`.
    pub unresolved: Vec<RepairChat>,
    pub unresolved_total: u64,
}

fn busy(error: sqlx::Error) -> RepositoryError {
    let text = error.to_string();
    if text.contains("locked") || text.contains("busy") {
        RepositoryError::Unavailable(
            "the database is locked by another process (is `serve` or a sync running?); retry later"
                .into(),
        )
    } else {
        storage_error(text)
    }
}

impl SqliteStore {
    /// Fixes NULL senders that can be derived from the archive alone. Idempotent; every batch of
    /// `batch_size` rows is its own transaction, so an interrupted run keeps its progress.
    pub async fn repair_senders(
        &self,
        dry_run: bool,
        batch_size: u32,
    ) -> Result<SenderRepairReport, RepositoryError> {
        let channels = sqlx::query("SELECT c.id, c.title FROM chats c WHERE c.kind='channel' AND EXISTS (SELECT 1 FROM messages m WHERE m.chat_id=c.id AND m.sender_id IS NULL) ORDER BY c.id")
            .fetch_all(&self.pool)
            .await
            .map_err(busy)?;
        let mut channel_posts = Vec::new();
        for row in channels {
            let chat_id: i64 = row.try_get("id").map_err(storage_error)?;
            let rows = if dry_run {
                let count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM messages WHERE chat_id=? AND sender_id IS NULL",
                )
                .bind(chat_id)
                .fetch_one(&self.pool)
                .await
                .map_err(busy)?;
                count as u64
            } else {
                let mut total = 0u64;
                loop {
                    let mut tx = self.pool.begin().await.map_err(busy)?;
                    let done = sqlx::query("UPDATE messages SET sender_id=chat_id, updated_at=unixepoch() WHERE row_id IN (SELECT row_id FROM messages WHERE chat_id=? AND sender_id IS NULL LIMIT ?)")
                        .bind(chat_id)
                        .bind(i64::from(batch_size.max(1)))
                        .execute(&mut *tx)
                        .await
                        .map_err(busy)?
                        .rows_affected();
                    tx.commit().await.map_err(busy)?;
                    total += done;
                    if done < u64::from(batch_size.max(1)) {
                        break;
                    }
                }
                total
            };
            if rows > 0 {
                channel_posts.push(RepairChat {
                    chat_id,
                    title: row.try_get("title").map_err(storage_error)?,
                    kind: "channel".into(),
                    rows,
                });
            }
        }
        let rows = sqlx::query("SELECT m.chat_id, c.title, COALESCE(c.kind, 'unknown') AS kind, COUNT(*) AS n FROM messages m LEFT JOIN chats c ON c.id=m.chat_id WHERE m.sender_id IS NULL AND COALESCE(c.kind, '') != 'channel' GROUP BY m.chat_id ORDER BY m.chat_id")
            .fetch_all(&self.pool)
            .await
            .map_err(busy)?;
        let mut unresolved = Vec::new();
        for row in rows {
            unresolved.push(RepairChat {
                chat_id: row.try_get("chat_id").map_err(storage_error)?,
                title: row.try_get("title").map_err(storage_error)?,
                kind: row.try_get("kind").map_err(storage_error)?,
                rows: row.try_get::<i64, _>("n").map_err(storage_error)? as u64,
            });
        }
        Ok(SenderRepairReport {
            dry_run,
            channel_posts_total: channel_posts.iter().map(|c| c.rows).sum(),
            channel_posts,
            unresolved_total: unresolved.iter().map(|c| c.rows).sum(),
            unresolved,
        })
    }
}

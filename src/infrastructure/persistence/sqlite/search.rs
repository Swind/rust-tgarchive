//! Full-text index maintenance (contentless FTS5 table `messages_fts`, rowid == `messages.row_id`)
//! and index state. Query building lives in `store.rs` next to the other message queries.

use sqlx::{Row, Sqlite, SqliteConnection, Transaction};

use crate::{
    application::{RepositoryError, SearchIndexState, SearchIndexStatus},
    infrastructure::search::{SEARCH_INDEX_VERSION, SearchTokenizer},
};

use super::{SqliteStore, storage_error};

const KEY_STATE: &str = "search_index_state";
const KEY_VERSION: &str = "search_index_version";
const INDEXABLE: &str = "text IS NOT NULL AND text <> ''";

/// Writes (replacing any previous entry) or removes the index entry of one message. Must run in
/// the same transaction as the message write.
pub(super) async fn index_message(
    tx: &mut Transaction<'_, Sqlite>,
    tokenizer: &dyn SearchTokenizer,
    row_id: i64,
    text: Option<&str>,
) -> Result<(), RepositoryError> {
    match text.filter(|text| !text.is_empty()) {
        Some(text) => {
            let doc = tokenizer.build_document(text);
            sqlx::query(
                "INSERT OR REPLACE INTO messages_fts(rowid, words, bigrams) VALUES (?, ?, ?)",
            )
            .bind(row_id)
            .bind(doc.words)
            .bind(doc.bigrams)
            .execute(&mut **tx)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM messages_fts WHERE rowid=?")
                .bind(row_id)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

async fn has_metadata_table(conn: &mut SqliteConnection) -> Result<bool, RepositoryError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_metadata'",
    )
    .fetch_one(conn)
    .await?
        > 0)
}

/// Recorded index version and state. A database that predates the metadata table (opened
/// read-only without migrating) is `Stale`, as is any version other than the current one.
pub(super) async fn read_state(
    conn: &mut SqliteConnection,
) -> Result<(u32, SearchIndexState), RepositoryError> {
    if !has_metadata_table(conn).await? {
        return Ok((0, SearchIndexState::Stale));
    }
    let rows = sqlx::query("SELECT key, value FROM app_metadata WHERE key IN (?, ?)")
        .bind(KEY_STATE)
        .bind(KEY_VERSION)
        .fetch_all(&mut *conn)
        .await?;
    let (mut state, mut version) = (SearchIndexState::Stale, 0);
    for row in rows {
        let key: String = row.try_get("key").map_err(storage_error)?;
        let value: String = row.try_get("value").map_err(storage_error)?;
        if key == KEY_STATE {
            state = match value.as_str() {
                "ready" => SearchIndexState::Ready,
                "rebuilding" => SearchIndexState::Rebuilding,
                _ => SearchIndexState::Stale,
            };
        } else {
            version = value.parse().unwrap_or(0);
        }
    }
    if state == SearchIndexState::Ready && version != SEARCH_INDEX_VERSION {
        state = SearchIndexState::Stale;
    }
    Ok((version, state))
}

async fn set_metadata(
    conn: &mut SqliteConnection,
    key: &str,
    value: &str,
) -> Result<(), RepositoryError> {
    sqlx::query("INSERT INTO app_metadata(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
        .bind(key)
        .bind(value)
        .execute(conn)
        .await?;
    Ok(())
}

/// Progress callback payload of [`SqliteStore::rebuild_search_index`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildProgress {
    pub indexed: u64,
    pub total: u64,
}

impl SqliteStore {
    pub async fn search_index_status(&self) -> Result<SearchIndexStatus, RepositoryError> {
        let mut conn = self.pool.acquire().await?;
        let (version, state) = read_state(&mut conn).await?;
        let total: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM messages WHERE {INDEXABLE}"))
                .fetch_one(&mut *conn)
                .await?;
        let indexed: i64 = if has_metadata_table(&mut conn).await? {
            sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts_docsize")
                .fetch_one(&mut *conn)
                .await?
        } else {
            0
        };
        Ok(SearchIndexStatus {
            version,
            state,
            indexed: u64::try_from(indexed).unwrap_or(0),
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Whether the stored index matches the current tokenization semantics.
    pub async fn search_index_ready(&self) -> Result<bool, RepositoryError> {
        let mut conn = self.pool.acquire().await?;
        Ok(read_state(&mut conn).await?.1 == SearchIndexState::Ready)
    }

    /// Drops every index entry and re-tokenizes all messages in batches of `batch_size` (one
    /// transaction each, so concurrent writers are only briefly blocked). While running the state
    /// is `rebuilding` and searches use the `LIKE` fallback; a crash leaves it `rebuilding`
    /// (shown as not ready) until the next rebuild.
    pub async fn rebuild_search_index(
        &self,
        batch_size: u32,
        mut progress: impl FnMut(RebuildProgress),
    ) -> Result<RebuildProgress, RepositoryError> {
        let batch_size = i64::from(batch_size.max(1));
        let mut tx = self.pool.begin().await?;
        set_metadata(&mut tx, KEY_STATE, "rebuilding").await?;
        // 'delete-all' is not available for contentless_delete tables.
        sqlx::query("DELETE FROM messages_fts")
            .execute(&mut *tx)
            .await?;
        let total: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM messages WHERE {INDEXABLE}"))
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;

        let total = u64::try_from(total).unwrap_or(0);
        let mut indexed = 0u64;
        let mut last_row = i64::MIN;
        progress(RebuildProgress { indexed, total });
        loop {
            let mut tx = self.pool.begin().await?;
            // Take the write lock first so the batch is read and written in one snapshot.
            set_metadata(&mut tx, KEY_STATE, "rebuilding").await?;
            let rows = sqlx::query(&format!(
                "SELECT row_id, text FROM messages WHERE row_id > ? AND {INDEXABLE} ORDER BY row_id LIMIT ?"
            ))
            .bind(last_row)
            .bind(batch_size)
            .fetch_all(&mut *tx)
            .await?;
            if rows.is_empty() {
                tx.rollback().await?;
                break;
            }
            for row in &rows {
                let row_id: i64 = row.try_get("row_id").map_err(storage_error)?;
                let text: String = row.try_get("text").map_err(storage_error)?;
                index_message(&mut tx, self.tokenizer.as_ref(), row_id, Some(&text)).await?;
                last_row = row_id;
            }
            tx.commit().await?;
            indexed += rows.len() as u64;
            progress(RebuildProgress { indexed, total });
        }

        let mut tx = self.pool.begin().await?;
        set_metadata(&mut tx, KEY_VERSION, &SEARCH_INDEX_VERSION.to_string()).await?;
        set_metadata(&mut tx, KEY_STATE, "ready").await?;
        tx.commit().await?;
        Ok(RebuildProgress { indexed, total })
    }
}

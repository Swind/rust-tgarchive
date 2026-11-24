use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Serialize};
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions, types::Json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, sqlx::FromRow)]
pub struct SearchGroup {
    pub id: i64,
    pub name: String,
    #[sqlx(json)]
    pub chat_ids: Vec<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchGroupError {
    #[error("invalid search group ID, name or members")]
    InvalidInput,
    #[error("search group was not found")]
    NotFound,
    #[error("search group storage: {0}")]
    Storage(#[from] sqlx::Error),
}

#[derive(Clone)]
pub struct SearchGroupStore {
    pool: SqlitePool,
}

impl SearchGroupStore {
    pub async fn open(path: &Path) -> Result<Self, SearchGroupError> {
        // An absolute filename cannot be reinterpreted as a file: URI by SQLite.
        let path = std::path::absolute(path).map_err(sqlx::Error::Io)?;
        let options = SqliteConnectOptions::new().filename(path);
        let pool = match SqlitePool::connect_with(options.clone().create_if_missing(true)).await {
            Ok(pool) => pool,
            Err(_) => SqlitePool::connect_with(options.read_only(true)).await?,
        };
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='search_groups')",
        )
        .fetch_one(&pool)
        .await?;
        if !exists {
            // Serialize first open across processes. Never initialize an unrelated SQLite DB.
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
            let occupied: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT IN ('search_groups', 'sqlite_sequence'))",
            )
            .fetch_one(&mut *tx)
            .await?;
            if occupied {
                return Err(
                    sqlx::Error::Configuration("not a search group database".into()).into(),
                );
            }
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS search_groups (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     name TEXT NOT NULL,
                     chat_ids TEXT NOT NULL CHECK(json_type(chat_ids) = 'array' AND json_array_length(chat_ids) <= 500)
                 )",
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
        }
        Ok(Self { pool })
    }

    pub async fn list(&self) -> Result<Vec<SearchGroup>, SearchGroupError> {
        Ok(
            sqlx::query_as("SELECT id, name, chat_ids FROM search_groups ORDER BY id")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    pub async fn get(&self, id: i64) -> Result<SearchGroup, SearchGroupError> {
        validate_id(id)?;
        sqlx::query_as("SELECT id, name, chat_ids FROM search_groups WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(SearchGroupError::NotFound)
    }

    // Membership is always read and replaced as a whole: keep it with the name so each write is
    // one SQLite statement transaction. AUTOINCREMENT prevents stale group links from being reused.
    pub async fn create(
        &self,
        name: &str,
        chat_ids: Vec<i64>,
    ) -> Result<SearchGroup, SearchGroupError> {
        let name = validate_group(name, &chat_ids)?;
        let result = sqlx::query("INSERT INTO search_groups (name, chat_ids) VALUES (?, ?)")
            .bind(name)
            .bind(Json(&chat_ids))
            .execute(&self.pool)
            .await?;
        Ok(SearchGroup {
            id: result.last_insert_rowid(),
            name: name.to_owned(),
            chat_ids,
        })
    }

    pub async fn update(
        &self,
        id: i64,
        name: &str,
        chat_ids: Vec<i64>,
    ) -> Result<SearchGroup, SearchGroupError> {
        validate_id(id)?;
        let name = validate_group(name, &chat_ids)?;
        let result = sqlx::query("UPDATE search_groups SET name = ?, chat_ids = ? WHERE id = ?")
            .bind(name)
            .bind(Json(&chat_ids))
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(SearchGroupError::NotFound);
        }
        Ok(SearchGroup {
            id,
            name: name.to_owned(),
            chat_ids,
        })
    }

    pub async fn delete(&self, id: i64) -> Result<(), SearchGroupError> {
        validate_id(id)?;
        let result = sqlx::query("DELETE FROM search_groups WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(SearchGroupError::NotFound);
        }
        Ok(())
    }
}

fn validate_id(id: i64) -> Result<(), SearchGroupError> {
    if id <= 0 {
        Err(SearchGroupError::InvalidInput)
    } else {
        Ok(())
    }
}

fn validate_group<'n>(name: &'n str, chat_ids: &[i64]) -> Result<&'n str, SearchGroupError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || chat_ids.len() > 500 {
        return Err(SearchGroupError::InvalidInput);
    }
    let mut seen = HashSet::with_capacity(chat_ids.len());
    if chat_ids.iter().any(|&id| id == 0 || !seen.insert(id)) {
        return Err(SearchGroupError::InvalidInput);
    }
    Ok(name)
}

#[cfg(test)]
mod tests;

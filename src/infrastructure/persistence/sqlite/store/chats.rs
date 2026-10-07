use super::*;

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

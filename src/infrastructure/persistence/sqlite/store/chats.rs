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

fn row_topic(row: SqliteRow) -> Result<TopicSummary, RepositoryError> {
    Ok(TopicSummary {
        id: MessageId::new(row.try_get("topic_id")?).map_err(invalid_data)?,
        title: row.try_get("title")?,
        collected: row.try_get("collected")?,
    })
}

#[async_trait::async_trait]
impl ChatRepository for SqliteStore {
    async fn save_topics(
        &self,
        chat_id: ChatId,
        topics: Vec<ForumTopic>,
    ) -> Result<(), RepositoryError> {
        // One snapshot parameter avoids SQLite's variable limit even for large forums.
        let snapshot = serde_json::to_string(&topics).map_err(invalid_data)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE forum_topics SET active=0, backfill_pending=0, before_id=NULL, generation=generation+1 WHERE chat_id=? AND active=1 AND topic_id NOT IN (SELECT value->>'id' FROM json_each(?))")
            .bind(chat_id.get())
            .bind(&snapshot)
            .execute(&mut *tx)
            .await?;
        if !topics.is_empty() {
            let repair = sqlx::query(
                "UPDATE chats SET topic_repair_needed=0 WHERE id=? AND topic_repair_needed=1",
            )
            .bind(chat_id.get())
            .execute(&mut *tx)
            .await?
            .rows_affected()
                != 0;
            sqlx::query("INSERT INTO forum_topics(chat_id,topic_id,title,backfill_pending) SELECT ?1,value->>'id',value->>'title',?2 FROM json_each(?3) WHERE true ON CONFLICT(chat_id,topic_id) DO UPDATE SET title=excluded.title, active=1 WHERE title != excluded.title OR active=0")
                .bind(chat_id.get()).bind(repair).bind(snapshot).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_topics(&self, chat_id: ChatId) -> Result<Vec<TopicSummary>, RepositoryError> {
        if self.pre_0010_topics {
            return Ok(Vec::new());
        }
        sqlx::query("SELECT topic_id,title,collected FROM forum_topics WHERE chat_id=? AND active=1 ORDER BY topic_id")
            .bind(chat_id.get())
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(row_topic)
            .collect()
    }

    async fn set_topic_collected(
        &self,
        chat_id: ChatId,
        topic_id: MessageId,
        collected: bool,
    ) -> Result<Option<TopicSummary>, RepositoryError> {
        // Repeated PUTs preserve a pending cursor. A change invalidates every outstanding page.
        sqlx::query("UPDATE forum_topics SET generation=generation+(collected != ?1), backfill_pending=CASE WHEN collected != ?1 THEN ?1 ELSE backfill_pending END, before_id=CASE WHEN collected = ?1 THEN before_id END, collected=?1 WHERE chat_id=?2 AND topic_id=?3 AND active=1 AND EXISTS(SELECT 1 FROM chats WHERE id=?2 AND tracked=1) RETURNING topic_id,title,collected")
            .bind(collected)
            .bind(chat_id.get())
            .bind(topic_id.get())
            .fetch_optional(&self.pool)
            .await?
            .map(row_topic)
            .transpose()
    }

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
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE chats SET tracked=?1, tracked_at=CASE WHEN ?1=1 THEN COALESCE(tracked_at, unixepoch()) ELSE NULL END WHERE id=?2 AND tracked != ?1")
            .bind(i64::from(tracked))
            .bind(id.get())
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if changed != 0 {
            // Pause/resume pending walks at their saved cursor. Tracking a chat does not
            // itself request historical backfill, but must invalidate in-flight pages.
            sqlx::query("UPDATE forum_topics SET generation=generation+1 WHERE chat_id=? AND backfill_pending=1")
                .bind(id.get())
                .execute(&mut *tx)
                .await?;
        }
        let row = sqlx::query("SELECT id, kind, title, username, tracked FROM chats WHERE id=?")
            .bind(id.get())
            .fetch_optional(&mut *tx)
            .await?;
        tx.commit().await?;
        row.as_ref().map(row_chat).transpose()
    }
}

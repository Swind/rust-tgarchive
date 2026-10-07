use super::*;

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

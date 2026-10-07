use super::*;

pub(super) fn transition_allowed(from: &SyncJobState, to: &SyncJobState) -> bool {
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

pub(super) fn job_state(state: SyncJobState) -> &'static str {
    match state {
        SyncJobState::Queued => "queued",
        SyncJobState::Running => "running",
        SyncJobState::Succeeded => "succeeded",
        SyncJobState::Failed => "failed",
        SyncJobState::Interrupted => "interrupted",
        SyncJobState::RateLimited => "rate_limited",
    }
}

pub(super) fn parse_job_state(value: &str) -> Result<SyncJobState, RepositoryError> {
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

pub(super) fn parse_scope(scope: &str, chat_id: Option<i64>) -> Result<SyncScope, RepositoryError> {
    match (scope, chat_id) {
        ("all", None) => Ok(SyncScope::All),
        ("chat", Some(id)) => Ok(SyncScope::Chat(
            ChatId::from_marked(id).map_err(invalid_data)?,
        )),
        _ => Err(invalid_data("invalid sync job scope")),
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
    async fn get_refetch_checkpoint(
        &self,
        chat_id: ChatId,
    ) -> Result<RefetchCheckpoint, RepositoryError> {
        let row = sqlx::query(
            "SELECT refetch_active, refetch_before_id FROM chat_sync_state WHERE chat_id=?",
        )
        .bind(chat_id.get())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(RefetchCheckpoint::default());
        };
        let before: Option<i64> = row.try_get("refetch_before_id").map_err(storage_error)?;
        Ok(RefetchCheckpoint {
            active: row
                .try_get::<i64, _>("refetch_active")
                .map_err(storage_error)?
                != 0,
            before_id: before
                .map(MessageId::new)
                .transpose()
                .map_err(invalid_data)?,
        })
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

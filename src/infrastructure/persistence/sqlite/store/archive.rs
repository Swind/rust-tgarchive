use super::sync::{job_state, parse_job_state, transition_allowed};
use super::*;

/// Upserts a sender and maintains `sender_name_history` in the same transaction.
///
/// `observed_at` is when tgarchive saw the profile (not when the user changed it). After the
/// COALESCE merge (NULL never clears a field), a changed display name or username appends a
/// history row for the merged values; unchanged values only advance `last_seen_at` of the
/// newest row (never backwards). Rows are never reordered or rewritten.
async fn save_sender(
    tx: &mut Transaction<'_, Sqlite>,
    sender: &Sender,
    observed_at: i64,
) -> Result<(), RepositoryError> {
    let old = sqlx::query("SELECT display_name, username FROM senders WHERE id=?")
        .bind(sender.id.get())
        .fetch_optional(&mut **tx)
        .await?;
    let (old_name, old_username): (Option<String>, Option<String>) = match &old {
        Some(row) => (
            row.try_get("display_name").map_err(storage_error)?,
            row.try_get("username").map_err(storage_error)?,
        ),
        None => (None, None),
    };
    sqlx::query("INSERT INTO senders(id, kind, display_name, username, is_bot, created_at, updated_at) VALUES (?, ?, ?, ?, ?, unixepoch(), unixepoch()) ON CONFLICT(id) DO UPDATE SET kind=CASE WHEN excluded.kind='unknown' THEN senders.kind ELSE excluded.kind END, display_name=COALESCE(excluded.display_name, senders.display_name), username=COALESCE(excluded.username, senders.username), is_bot=COALESCE(excluded.is_bot, senders.is_bot), updated_at=unixepoch()")
        .bind(sender.id.get()).bind(sender_kind(sender.kind)).bind(&sender.display_name).bind(&sender.username).bind(sender.is_bot)
        .execute(&mut **tx).await?;
    let name = sender.display_name.clone().or(old_name.clone());
    let username = sender.username.clone().or(old_username.clone());
    if name.is_none() && username.is_none() {
        return Ok(());
    }
    if name != old_name || username != old_username {
        sqlx::query("INSERT INTO sender_name_history(sender_id, display_name, username, first_seen_at, last_seen_at) VALUES (?, ?, ?, ?, ?)")
            .bind(sender.id.get()).bind(&name).bind(&username).bind(observed_at).bind(observed_at)
            .execute(&mut **tx).await?;
    } else {
        let updated = sqlx::query("UPDATE sender_name_history SET last_seen_at=MAX(last_seen_at, ?) WHERE id=(SELECT MAX(id) FROM sender_name_history WHERE sender_id=?)")
            .bind(observed_at).bind(sender.id.get())
            .execute(&mut **tx).await?
            .rows_affected();
        if updated == 0 {
            sqlx::query("INSERT INTO sender_name_history(sender_id, display_name, username, first_seen_at, last_seen_at) VALUES (?, ?, ?, ?, ?)")
                .bind(sender.id.get()).bind(&name).bind(&username).bind(observed_at).bind(observed_at)
                .execute(&mut **tx).await?;
        }
    }
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
    if old.is_none() {
        sqlx::query("UPDATE chat_sync_state SET last_sync_started_at=unixepoch() WHERE chat_id=?")
            .bind(progress.chat_id.get())
            .execute(&mut **tx)
            .await?;
    }
    if progress.state == SyncJobState::Succeeded {
        sqlx::query(
            "UPDATE chat_sync_state SET last_sync_completed_at=unixepoch() WHERE chat_id=?",
        )
        .bind(progress.chat_id.get())
        .execute(&mut **tx)
        .await?;
    }
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

/// Fills only NULL columns of an existing row (never text, versions or deletion state), so a
/// re-fetch cannot overwrite or resurrect anything.
async fn backfill_metadata(
    tx: &mut Transaction<'_, Sqlite>,
    row_id: i64,
    message: &Message,
) -> Result<(), RepositoryError> {
    let forward = message.forward.as_ref();
    let fwd_id = forward.and_then(|f| f.from_id).map(SenderId::get);
    let fwd_name = forward.and_then(|f| f.from_name.clone());
    let fwd_date = forward.and_then(|f| f.date).map(seconds);
    sqlx::query("UPDATE messages SET sender_id=COALESCE(sender_id, ?1), post_author=COALESCE(post_author, ?2), fwd_from_id=COALESCE(fwd_from_id, ?3), fwd_from_name=COALESCE(fwd_from_name, ?4), fwd_date=COALESCE(fwd_date, ?5), updated_at=unixepoch() WHERE row_id=?6 AND ((sender_id IS NULL AND ?1 IS NOT NULL) OR (post_author IS NULL AND ?2 IS NOT NULL) OR (fwd_from_id IS NULL AND ?3 IS NOT NULL) OR (fwd_from_name IS NULL AND ?4 IS NOT NULL) OR (fwd_date IS NULL AND ?5 IS NOT NULL))")
        .bind(message.sender_id.map(SenderId::get))
        .bind(&message.post_author)
        .bind(fwd_id)
        .bind(fwd_name)
        .bind(fwd_date)
        .bind(row_id)
        .execute(&mut **tx)
        .await?;
    for (ordinal, attachment) in message.attachments.iter().enumerate() {
        let updated = sqlx::query("UPDATE attachments SET telegram_file_id=COALESCE(telegram_file_id, ?), mime_type=COALESCE(mime_type, ?), file_name=COALESCE(file_name, ?), size=COALESCE(size, ?) WHERE message_row_id=? AND ordinal=?")
            .bind(&attachment.telegram_file_id).bind(&attachment.mime_type).bind(&attachment.file_name).bind(attachment.size)
            .bind(row_id).bind(ordinal as i64)
            .execute(&mut **tx).await?
            .rows_affected();
        if updated == 0 {
            sqlx::query("INSERT INTO attachments(message_row_id, ordinal, kind, telegram_file_id, mime_type, file_name, size) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .bind(row_id).bind(ordinal as i64).bind(attachment_kind(attachment.kind))
                .bind(&attachment.telegram_file_id).bind(&attachment.mime_type).bind(&attachment.file_name).bind(attachment.size)
                .execute(&mut **tx).await?;
        }
    }
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
        let observed_at = batch
            .records
            .iter()
            .filter_map(|record| match &record.event {
                MessageEvent::Created(message) | MessageEvent::Updated(message) => {
                    Some(seconds(message.collected_at))
                }
                MessageEvent::Deleted { .. } => None,
            })
            .max()
            .unwrap_or_else(|| Utc::now().timestamp());
        for sender in &batch.senders {
            save_sender(&mut tx, sender, observed_at).await?;
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
                    let priority = i64::from(matches!(record.source, MessageSource::Realtime));
                    let previous: Option<(i64, Option<String>)> = sqlx::query_as(
                        "SELECT row_id, text FROM messages WHERE chat_id=? AND message_id=?",
                    )
                    .bind(message.chat_id.get())
                    .bind(message.id.get())
                    .fetch_optional(&mut *tx)
                    .await?;
                    if let (MessageSource::Refetch, Some((row_id, _))) = (record.source, &previous)
                    {
                        backfill_metadata(&mut tx, *row_id, message).await?;
                        enqueue_attachment_downloads(
                            &mut tx,
                            message.chat_id.get(),
                            message.id.get(),
                            *row_id,
                        )
                        .await?;
                        continue;
                    }
                    let result = sqlx::query("INSERT INTO messages(chat_id, message_id, sender_id, timestamp, edited_at, collected_at, created_at, updated_at, text, reply_to, version_at, source_priority, post_author, fwd_from_id, fwd_from_name, fwd_date) SELECT ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch(), ?, ?, ?, ?, ?, ?, ?, ? WHERE NOT EXISTS(SELECT 1 FROM message_tombstones WHERE chat_id=? AND message_id=?) AND NOT (? > -1000000000000 AND EXISTS(SELECT 1 FROM common_message_tombstones WHERE message_id=?)) ON CONFLICT(chat_id, message_id) DO UPDATE SET sender_id=COALESCE(excluded.sender_id, messages.sender_id), post_author=COALESCE(excluded.post_author, messages.post_author), fwd_from_id=COALESCE(excluded.fwd_from_id, messages.fwd_from_id), fwd_from_name=COALESCE(excluded.fwd_from_name, messages.fwd_from_name), fwd_date=COALESCE(excluded.fwd_date, messages.fwd_date), timestamp=excluded.timestamp, edited_at=excluded.edited_at, collected_at=excluded.collected_at, updated_at=unixepoch(), text=excluded.text, reply_to=excluded.reply_to, version_at=excluded.version_at, source_priority=excluded.source_priority WHERE (excluded.version_at > messages.version_at OR (excluded.version_at = messages.version_at AND excluded.source_priority >= messages.source_priority)) AND messages.is_deleted=0")
                        .bind(message.chat_id.get()).bind(message.id.get()).bind(message.sender_id.map(SenderId::get))
                        .bind(seconds(message.timestamp)).bind(message.edited_at.map(seconds)).bind(seconds(message.collected_at))
                        .bind(&message.text).bind(message.reply_to.map(MessageId::get)).bind(version).bind(priority)
                        .bind(&message.post_author)
                        .bind(message.forward.as_ref().and_then(|f| f.from_id).map(SenderId::get))
                        .bind(message.forward.as_ref().and_then(|f| f.from_name.clone()))
                        .bind(message.forward.as_ref().and_then(|f| f.date).map(seconds))
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
                        // The guarded upsert changed the row: re-index only if the text did.
                        let new_text = message.text.as_deref().filter(|text| !text.is_empty());
                        let old_text = previous
                            .as_ref()
                            .and_then(|(_, text)| text.as_deref())
                            .filter(|text| !text.is_empty());
                        if new_text != old_text {
                            index_message(&mut tx, self.tokenizer.as_ref(), row_id, new_text)
                                .await?;
                        }
                        replace_attachments(&mut tx, row_id, &message.attachments).await?;
                        enqueue_attachment_downloads(
                            &mut tx,
                            message.chat_id.get(),
                            message.id.get(),
                            row_id,
                        )
                        .await?;
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
        if let Some((chat_id, refetch)) = batch.refetch_checkpoint {
            ensure_chat(&mut tx, chat_id).await?;
            sqlx::query("UPDATE chat_sync_state SET refetch_active=?, refetch_before_id=?, updated_at=unixepoch() WHERE chat_id=?")
                .bind(refetch.active)
                .bind(refetch.before_id.map(MessageId::get))
                .bind(chat_id.get())
                .execute(&mut *tx)
                .await?;
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

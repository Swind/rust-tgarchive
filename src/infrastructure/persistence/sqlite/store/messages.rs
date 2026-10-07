use super::*;

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

/// `legacy` is a read-only database that predates migration 0007: the metadata columns read as NULL.
fn message_select(legacy: LegacyColumns) -> String {
    let metadata = if legacy.metadata {
        "NULL AS post_author, NULL AS fwd_from_id, NULL AS fwd_from_name, NULL AS fwd_date"
    } else {
        "m.post_author, m.fwd_from_id, m.fwd_from_name, m.fwd_date"
    };
    let bot = if legacy.bot { "NULL" } else { "s.is_bot" };
    format!("{MESSAGE_SELECT_HEAD}{metadata}{MESSAGE_SELECT_MID}{bot}{MESSAGE_SELECT_TAIL}")
}

const MESSAGE_SELECT_HEAD: &str = "SELECT m.row_id, m.chat_id, m.message_id, m.sender_id, m.timestamp, m.edited_at, m.collected_at, m.text, m.reply_to, ";
const MESSAGE_SELECT_MID: &str = ", CASE WHEN m.is_deleted=1 THEN COALESCE(m.deleted_at, 0) END AS deleted_at, s.display_name AS sender_name, s.username AS sender_username, ";
const MESSAGE_SELECT_TAIL: &str = " AS sender_is_bot, EXISTS(SELECT 1 FROM telegram_account_identity i WHERE i.user_id=m.sender_id) AS sender_is_self, c.title AS chat_title FROM messages m LEFT JOIN senders s ON s.id=m.sender_id LEFT JOIN chats c ON c.id=m.chat_id";

fn row_message(row: &SqliteRow, attachments: Vec<Attachment>) -> Result<Message, RepositoryError> {
    let sender: Option<i64> = row.try_get("sender_id").map_err(storage_error)?;
    let edited: Option<i64> = row.try_get("edited_at").map_err(storage_error)?;
    let reply: Option<i64> = row.try_get("reply_to").map_err(storage_error)?;
    let fwd_id: Option<i64> = row.try_get("fwd_from_id").map_err(storage_error)?;
    let fwd_name: Option<String> = row.try_get("fwd_from_name").map_err(storage_error)?;
    let fwd_date: Option<i64> = row.try_get("fwd_date").map_err(storage_error)?;
    let forward = if fwd_id.is_some() || fwd_name.is_some() || fwd_date.is_some() {
        Some(Forward {
            from_id: fwd_id
                .map(SenderId::from_marked)
                .transpose()
                .map_err(invalid_data)?,
            from_name: fwd_name,
            date: fwd_date.map(timestamp).transpose()?,
        })
    } else {
        None
    };
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
        post_author: row.try_get("post_author").map_err(storage_error)?,
        forward,
    })
}

fn row_view(row: &SqliteRow, attachments: Vec<Attachment>) -> Result<MessageView, RepositoryError> {
    let message = row_message(row, attachments)?;
    let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(storage_error)?;
    let name: Option<String> = row.try_get("sender_name").map_err(storage_error)?;
    let username: Option<String> = row.try_get("sender_username").map_err(storage_error)?;
    let chat_title: Option<String> = row.try_get("chat_title").map_err(storage_error)?;
    let is_bot: Option<bool> = row.try_get("sender_is_bot").map_err(storage_error)?;
    let is_self: bool = row.try_get("sender_is_self").map_err(storage_error)?;
    let sender = message.sender_id.map(|id| SenderInfo {
        id,
        // A chat/channel posting as itself has no profile name of its own: use the chat title.
        display_name: name.or_else(|| {
            (id.get() == message.chat_id.get())
                .then_some(chat_title)
                .flatten()
        }),
        username,
        is_bot,
        is_self,
    });
    Ok(MessageView {
        message,
        sender,
        deleted_at: deleted_at.map(timestamp).transpose()?,
        snippet: None,
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

/// Text-search restrictions added to the message listing query.
#[derive(Default)]
struct SearchSpec {
    /// FTS5 `MATCH` expression (index must be ready).
    fts: Option<String>,
    /// Substrings `m.text` must contain.
    like_terms: Vec<String>,
    /// Relevance paging: skip this many results and order by rank (FTS) or newest first (LIKE)
    /// instead of using keyset cursors.
    offset: Option<u64>,
}

fn like_pattern(term: &str) -> String {
    let mut pattern = String::from("%");
    for ch in term.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

async fn list_messages(
    tx: &mut Transaction<'_, Sqlite>,
    filters: &crate::application::MessageFilters,
    before: Option<&crate::application::MessageCursor>,
    after: Option<&crate::application::MessageCursor>,
    page_size: crate::application::PageSize,
    search: Option<&SearchSpec>,
    legacy: LegacyColumns,
) -> Result<crate::application::MessagePage, RepositoryError> {
    let after_direction = after.is_some();
    let mut query = QueryBuilder::<Sqlite>::new(message_select(legacy));
    let fts = search.is_some_and(|spec| spec.fts.is_some());
    if fts {
        query.push(" JOIN messages_fts ON messages_fts.rowid=m.row_id");
    }
    // With a MATCH the FTS hits must drive the query. Without statistics the planner would
    // otherwise walk the chat/sender index and probe the FTS table once per message (O(messages
    // in the chat), 100-400 ms at 100k); the unary `+` keeps those columns from being indexed.
    let plus = if fts { "+" } else { "" };
    query.push(if filters.include_deleted {
        " WHERE 1=1"
    } else {
        " WHERE m.is_deleted=0"
    });
    if let Some(chat_id) = filters.chat_id {
        query
            .push(format!(" AND {plus}m.chat_id="))
            .push_bind(chat_id.get());
    }
    if let Some(sender_id) = filters.sender_id {
        query
            .push(format!(" AND {plus}m.sender_id="))
            .push_bind(sender_id.get());
    }
    if filters.exclude_bots && !legacy.bot {
        query.push(" AND (s.is_bot IS NULL OR s.is_bot=0)");
    }
    if let Some(author) = &filters.post_author {
        query
            .push(format!(" AND {plus}m.post_author="))
            .push_bind(author.clone());
    }
    if let Some(from) = filters.time_range.from {
        query
            .push(" AND m.timestamp>=")
            .push_bind(ceil_seconds(from)?);
    }
    if let Some(to) = filters.time_range.to {
        query.push(" AND m.timestamp<").push_bind(ceil_seconds(to)?);
    }
    if let Some(spec) = search {
        if let Some(expression) = &spec.fts {
            query.push(" AND messages_fts MATCH ").push_bind(expression);
        }
        for term in &spec.like_terms {
            query
                .push(" AND m.text LIKE ")
                .push_bind(like_pattern(term))
                .push(" ESCAPE '\\'");
        }
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
    let offset = search.and_then(|spec| spec.offset);
    if offset.is_some() && fts {
        query.push(" ORDER BY bm25(messages_fts, 5.0, 1.0), m.row_id LIMIT ");
    } else {
        query.push(if after_direction {
            " ORDER BY m.timestamp ASC, m.chat_id ASC, m.message_id ASC LIMIT "
        } else {
            " ORDER BY m.timestamp DESC, m.chat_id DESC, m.message_id DESC LIMIT "
        });
    }
    query.push_bind(i64::from(page_size.get()) + 1);
    if let Some(offset) = offset {
        query
            .push(" OFFSET ")
            .push_bind(i64::try_from(offset).map_err(invalid_data)?);
    }
    let mut rows = query.build().fetch_all(&mut **tx).await?;
    let has_more = rows.len() > usize::from(page_size.get());
    rows.truncate(usize::from(page_size.get()));
    if after_direction {
        rows.reverse();
    }
    let items = load_messages(tx, rows).await?;
    let next_offset = match offset {
        Some(offset) if has_more => Some(offset + u64::from(page_size.get())),
        _ => None,
    };
    let next_cursor = if has_more && offset.is_none() {
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
        next_offset,
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
            "{} WHERE m.chat_id=? AND m.message_id=? AND (m.is_deleted=0 OR ?)",
            message_select(self.legacy())
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
            self.legacy(),
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
        let plan = plan_query(self.tokenizer.as_ref(), &query.text);
        if plan.is_empty() {
            return Err(invalid_data("search query cannot be empty"));
        }
        let relevance = query.sort == crate::application::SearchSort::Relevance;
        let mut tx = self.pool.begin().await?;
        let ready = read_state(&mut tx).await?.1 == crate::application::SearchIndexState::Ready;
        let spec = if ready {
            SearchSpec {
                fts: plan.fts.clone(),
                like_terms: plan.like_terms.clone(),
                offset: relevance.then_some(query.offset),
            }
        } else {
            // Index missing, outdated or being rebuilt: substring scan over the source text.
            SearchSpec {
                fts: None,
                like_terms: plan.fallback_terms.clone(),
                offset: relevance.then_some(query.offset),
            }
        };
        let mut page = list_messages(
            &mut tx,
            &query.filters,
            query.before.as_ref(),
            query.after.as_ref(),
            query.page_size,
            Some(&spec),
            self.legacy(),
        )
        .await?;
        tx.commit().await.map_err(storage_error)?;
        for item in &mut page.items {
            item.snippet = item
                .message
                .text
                .as_deref()
                .and_then(|text| make_snippet(text, &plan.highlight));
        }
        Ok(page)
    }

    async fn search_index_status(
        &self,
    ) -> Result<crate::application::SearchIndexStatus, RepositoryError> {
        SqliteStore::search_index_status(self).await
    }

    async fn bound_account(&self) -> Result<Option<SenderId>, RepositoryError> {
        SqliteStore::bound_account(self).await
    }

    async fn search_senders(&self, query: SenderQuery) -> Result<SenderPage, RepositoryError> {
        self.search_senders_impl(query).await
    }

    async fn sender_detail(
        &self,
        id: SenderId,
        include_deleted: bool,
    ) -> Result<Option<SenderDetail>, RepositoryError> {
        self.sender_detail_impl(id, include_deleted).await
    }

    async fn list_senders(
        &self,
        chat_id: ChatId,
        limit: PageSize,
    ) -> Result<Vec<SenderSummary>, RepositoryError> {
        let bot = if self.pre_0008 { "NULL" } else { "s.is_bot" };
        let rows = sqlx::query(&format!("SELECT m.sender_id, COALESCE(s.display_name, CASE WHEN m.sender_id=m.chat_id THEN c.title END) AS display_name, s.username, {bot} AS is_bot, COUNT(*) AS message_count FROM messages m LEFT JOIN senders s ON s.id=m.sender_id LEFT JOIN chats c ON c.id=m.chat_id WHERE m.chat_id=? AND m.is_deleted=0 AND m.sender_id IS NOT NULL GROUP BY m.sender_id ORDER BY message_count DESC, m.sender_id LIMIT ?"))
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
                        is_bot: row.try_get("is_bot").map_err(storage_error)?,
                        is_self: false,
                    },
                    message_count: u64::try_from(count).map_err(invalid_data)?,
                })
            })
            .collect()
    }
}

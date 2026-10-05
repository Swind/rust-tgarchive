use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::{
    application::{
        RepositoryError, SenderChatStat, SenderDetail, SenderPage, SenderProfile, SenderQuery,
        SenderSort,
    },
    domain::{ChatId, SenderId, SenderKind},
    infrastructure::search::normalize_text,
};

use super::{
    storage_error,
    store::{SqliteStore, invalid_data, parse_chat_kind, timestamp},
};

/// Candidate lists up to this size use per-sender index seeks; larger ones aggregate globally.
const MAX_SEEK_CANDIDATES: usize = 2000;
const SEEK_CHUNK: usize = 500;

struct Info {
    display_name: Option<String>,
    username: Option<String>,
    kind: Option<SenderKind>,
}

struct Aggregate {
    messages: u64,
    chats: u64,
    first: Option<i64>,
    last: Option<i64>,
}

enum Matcher {
    All,
    Text {
        text: String,
        id: Option<i64>,
    },
    /// `@username`: exact, case-insensitive.
    Username(String),
}

impl Matcher {
    fn parse(text: Option<&str>) -> Self {
        let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
            return Self::All;
        };
        if let Some(name) = text.strip_prefix('@') {
            return Self::Username(normalize_text(name));
        }
        Self::Text {
            text: normalize_text(text),
            id: text.parse().ok(),
        }
    }

    fn matches(&self, id: i64, info: &Info) -> bool {
        let contains = |value: &Option<String>, needle: &str| {
            value
                .as_deref()
                .is_some_and(|value| normalize_text(value).contains(needle))
        };
        match self {
            Self::All => true,
            Self::Username(name) => info
                .username
                .as_deref()
                .is_some_and(|value| normalize_text(value) == *name),
            Self::Text { text, id: wanted } => {
                *wanted == Some(id)
                    || contains(&info.display_name, text)
                    || contains(&info.username, text)
            }
        }
    }
}

fn parse_sender_kind(value: &str) -> SenderKind {
    match value {
        "user" => SenderKind::User,
        "chat" => SenderKind::Chat,
        "channel" => SenderKind::Channel,
        _ => SenderKind::Unknown,
    }
}

fn inferred_kind(id: i64) -> SenderKind {
    if id > 0 {
        SenderKind::User
    } else if id > -1_000_000_000_000 {
        SenderKind::Chat
    } else {
        SenderKind::Channel
    }
}

impl SqliteStore {
    /// User the archive is bound to, if any.
    pub async fn bound_account(&self) -> Result<Option<SenderId>, RepositoryError> {
        let id: Option<i64> =
            sqlx::query_scalar("SELECT user_id FROM telegram_account_identity WHERE singleton=1")
                .fetch_optional(&self.pool)
                .await?;
        id.map(SenderId::from_marked)
            .transpose()
            .map_err(invalid_data)
    }

    /// Names of every known sender; a chat/channel posting as itself falls back to its title.
    async fn sender_infos(&self) -> Result<HashMap<i64, Info>, RepositoryError> {
        let mut infos = HashMap::new();
        let rows = sqlx::query("SELECT id, kind, display_name, username FROM senders")
            .fetch_all(&self.pool)
            .await?;
        for row in rows {
            let kind: String = row.try_get("kind").map_err(storage_error)?;
            infos.insert(
                row.try_get("id").map_err(storage_error)?,
                Info {
                    display_name: row.try_get("display_name").map_err(storage_error)?,
                    username: row.try_get("username").map_err(storage_error)?,
                    kind: Some(parse_sender_kind(&kind)),
                },
            );
        }
        let rows = sqlx::query("SELECT id, title, username FROM chats")
            .fetch_all(&self.pool)
            .await?;
        for row in rows {
            let id: i64 = row.try_get("id").map_err(storage_error)?;
            let title: Option<String> = row.try_get("title").map_err(storage_error)?;
            let username: Option<String> = row.try_get("username").map_err(storage_error)?;
            let info = infos.entry(id).or_insert(Info {
                display_name: None,
                username: None,
                kind: None,
            });
            if info.display_name.is_none() {
                info.display_name = title;
            }
            if info.username.is_none() {
                info.username = username;
            }
        }
        Ok(infos)
    }

    async fn aggregates(
        &self,
        ids: Option<&[i64]>,
        include_deleted: bool,
    ) -> Result<HashMap<i64, Aggregate>, RepositoryError> {
        let mut result = HashMap::new();
        let chunks: Vec<Option<&[i64]>> = match ids {
            Some(ids) if ids.len() <= MAX_SEEK_CANDIDATES => {
                ids.chunks(SEEK_CHUNK).map(Some).collect()
            }
            _ => vec![None],
        };
        let wanted: Option<HashSet<i64>> = match ids {
            Some(ids) if ids.len() > MAX_SEEK_CANDIDATES => Some(ids.iter().copied().collect()),
            _ => None,
        };
        for chunk in chunks {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT sender_id, COUNT(*) AS n, COUNT(DISTINCT chat_id) AS chats, MIN(timestamp) AS first_ts, MAX(timestamp) AS last_ts FROM messages WHERE sender_id IS NOT NULL",
            );
            if !include_deleted {
                query.push(" AND is_deleted=0");
            }
            if let Some(chunk) = chunk {
                query.push(" AND sender_id IN (");
                let mut separated = query.separated(", ");
                for id in chunk {
                    separated.push_bind(*id);
                }
                separated.push_unseparated(")");
            }
            query.push(" GROUP BY sender_id");
            for row in query.build().fetch_all(&self.pool).await? {
                let id: i64 = row.try_get("sender_id").map_err(storage_error)?;
                if wanted.as_ref().is_some_and(|wanted| !wanted.contains(&id)) {
                    continue;
                }
                let count = |name: &str| -> Result<u64, RepositoryError> {
                    u64::try_from(row.try_get::<i64, _>(name).map_err(storage_error)?)
                        .map_err(invalid_data)
                };
                result.insert(
                    id,
                    Aggregate {
                        messages: count("n")?,
                        chats: count("chats")?,
                        first: row.try_get("first_ts").map_err(storage_error)?,
                        last: row.try_get("last_ts").map_err(storage_error)?,
                    },
                );
            }
        }
        Ok(result)
    }

    fn profile(
        id: i64,
        info: Option<&Info>,
        aggregate: Option<&Aggregate>,
        account: Option<SenderId>,
    ) -> Result<SenderProfile, RepositoryError> {
        let time = |value: Option<i64>| -> Result<Option<DateTime<Utc>>, RepositoryError> {
            value.map(timestamp).transpose()
        };
        Ok(SenderProfile {
            id: SenderId::from_marked(id).map_err(invalid_data)?,
            kind: info
                .and_then(|info| info.kind)
                .filter(|kind| *kind != SenderKind::Unknown)
                .unwrap_or_else(|| inferred_kind(id)),
            display_name: info.and_then(|info| info.display_name.clone()),
            username: info.and_then(|info| info.username.clone()),
            message_count: aggregate.map_or(0, |a| a.messages),
            chat_count: aggregate.map_or(0, |a| a.chats),
            first_message_at: time(aggregate.and_then(|a| a.first))?,
            last_message_at: time(aggregate.and_then(|a| a.last))?,
            is_self: account.is_some_and(|account| account.get() == id),
        })
    }

    pub(super) async fn search_senders_impl(
        &self,
        query: SenderQuery,
    ) -> Result<SenderPage, RepositoryError> {
        let matcher = Matcher::parse(query.text.as_deref());
        let infos = self.sender_infos().await?;
        let account = self.bound_account().await?;
        let candidates: Option<Vec<i64>> = match matcher {
            Matcher::All => None,
            _ => {
                let mut ids: Vec<i64> = infos
                    .iter()
                    .filter(|(id, info)| matcher.matches(**id, info))
                    .map(|(id, _)| *id)
                    .collect();
                if let Matcher::Text { id: Some(id), .. } = &matcher
                    && !ids.contains(id)
                {
                    ids.push(*id);
                }
                Some(ids)
            }
        };
        let aggregates = self
            .aggregates(candidates.as_deref(), query.include_deleted)
            .await?;
        let mut profiles = Vec::with_capacity(aggregates.len());
        for (id, aggregate) in &aggregates {
            profiles.push(Self::profile(*id, infos.get(id), Some(aggregate), account)?);
        }
        match query.sort {
            SenderSort::Messages => profiles.sort_by(|a, b| {
                b.message_count
                    .cmp(&a.message_count)
                    .then(a.id.get().cmp(&b.id.get()))
            }),
            SenderSort::LastMessage => profiles.sort_by(|a, b| {
                b.last_message_at
                    .cmp(&a.last_message_at)
                    .then(a.id.get().cmp(&b.id.get()))
            }),
            SenderSort::Name => {
                profiles.sort_by_cached_key(|p| {
                    (
                        p.display_name.is_none(),
                        p.display_name.as_deref().map(normalize_text),
                        p.id.get(),
                    )
                });
            }
        }
        let offset = usize::try_from(query.offset).unwrap_or(usize::MAX);
        let limit = usize::from(query.limit.get());
        let total = profiles.len();
        let items: Vec<_> = profiles.into_iter().skip(offset).take(limit).collect();
        let next_offset =
            (offset.saturating_add(limit) < total).then(|| query.offset + limit as u64);
        Ok(SenderPage { items, next_offset })
    }

    pub(super) async fn sender_detail_impl(
        &self,
        id: SenderId,
        include_deleted: bool,
    ) -> Result<Option<SenderDetail>, RepositoryError> {
        let infos = self.sender_infos().await?;
        let account = self.bound_account().await?;
        let aggregates = self.aggregates(Some(&[id.get()]), include_deleted).await?;
        let known = infos
            .get(&id.get())
            .is_some_and(|info| info.kind.is_some() || info.username.is_some());
        if !known && !aggregates.contains_key(&id.get()) {
            return Ok(None);
        }
        let profile = Self::profile(
            id.get(),
            infos.get(&id.get()),
            aggregates.get(&id.get()),
            account,
        )?;
        let rows = sqlx::query(&format!(
            "SELECT m.chat_id, c.title, COALESCE(c.kind, 'group') AS kind, COUNT(*) AS n, MAX(m.timestamp) AS last_ts FROM messages m LEFT JOIN chats c ON c.id=m.chat_id WHERE m.sender_id=?{} GROUP BY m.chat_id ORDER BY n DESC, last_ts DESC, m.chat_id",
            if include_deleted { "" } else { " AND m.is_deleted=0" }
        ))
        .bind(id.get())
        .fetch_all(&self.pool)
        .await?;
        let chats = rows
            .iter()
            .map(|row| {
                let last: Option<i64> = row.try_get("last_ts").map_err(storage_error)?;
                Ok(SenderChatStat {
                    chat_id: ChatId::from_marked(row.try_get("chat_id").map_err(storage_error)?)
                        .map_err(invalid_data)?,
                    title: row.try_get("title").map_err(storage_error)?,
                    kind: parse_chat_kind(row.try_get("kind").map_err(storage_error)?)?,
                    message_count: u64::try_from(
                        row.try_get::<i64, _>("n").map_err(storage_error)?,
                    )
                    .map_err(invalid_data)?,
                    last_message_at: last.map(timestamp).transpose()?,
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        Ok(Some(SenderDetail { profile, chats }))
    }
}

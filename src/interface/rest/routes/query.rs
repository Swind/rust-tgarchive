use super::{ApiError, dto::SearchSortDto};
use crate::{
    application::{
        ListMessagesQuery, MessageFilters, PageSize, SearchMessagesQuery, SearchSort, TimeRange,
    },
    domain::{ChatId, SenderId},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::IntoParams;

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct MessageQuery {
    pub(super) chat_id: Option<i64>,
    pub(super) sender_id: Option<i64>,
    /// Exact channel post signature.
    pub(super) post_author: Option<String>,
    pub(super) from: Option<DateTime<Utc>>,
    pub(super) to: Option<DateTime<Utc>>,
    pub(super) before: Option<String>,
    pub(super) after: Option<String>,
    pub(super) limit: Option<u16>,
    /// Also return deleted messages (flagged with `is_deleted`/`deleted_at`).
    pub(super) include_deleted: Option<bool>,
    /// Hide messages from known bots (`senders.is_bot=1`); unknown senders are kept.
    pub(super) exclude_bots: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct SearchQuery {
    pub(super) q: String,
    pub(super) chat_id: Option<i64>,
    pub(super) sender_id: Option<i64>,
    /// Exact channel post signature.
    pub(super) post_author: Option<String>,
    pub(super) from: Option<DateTime<Utc>>,
    pub(super) to: Option<DateTime<Utc>>,
    /// Cursor from the previous page's `next_cursor`.
    pub(super) before: Option<String>,
    /// Only with `sort=time`: cursor for the newer direction.
    pub(super) after: Option<String>,
    pub(super) limit: Option<u16>,
    /// Also return deleted messages (flagged with `is_deleted`/`deleted_at`).
    pub(super) include_deleted: Option<bool>,
    /// Hide messages from known bots (`senders.is_bot=1`); unknown senders are kept.
    pub(super) exclude_bots: Option<bool>,
    /// Result order; default `relevance`.
    pub(super) sort: Option<SearchSortDto>,
}

pub(in crate::interface::rest) fn build_list_query(
    query: MessageQuery,
    request_id: String,
) -> Result<ListMessagesQuery, ApiError> {
    let page_size = PageSize::new(query.limit.unwrap_or(PageSize::DEFAULT.get()))
        .map_err(|error| ApiError::from_application(error.into(), request_id.clone()))?;
    let mut filters = filters(
        query.chat_id,
        query.sender_id,
        query.post_author,
        query.from,
        query.to,
        query.include_deleted,
        request_id.clone(),
    )?;
    filters.exclude_bots = query.exclude_bots.unwrap_or(false);
    let before = decode_cursor(query.before, request_id.clone())?;
    let after = decode_cursor(query.after, request_id.clone())?;
    let result = ListMessagesQuery {
        filters,
        before,
        after,
        page_size,
    };
    result
        .validate()
        .map_err(|error| ApiError::from_application(error.into(), request_id))?;
    Ok(result)
}

pub(in crate::interface::rest) fn build_search_query(
    query: SearchQuery,
    request_id: String,
) -> Result<SearchMessagesQuery, ApiError> {
    let page_size = PageSize::new(query.limit.unwrap_or(PageSize::DEFAULT.get()))
        .map_err(|error| ApiError::from_application(error.into(), request_id.clone()))?;
    let mut filters = filters(
        query.chat_id,
        query.sender_id,
        query.post_author,
        query.from,
        query.to,
        query.include_deleted,
        request_id.clone(),
    )?;
    filters.exclude_bots = query.exclude_bots.unwrap_or(false);
    let sort = SearchSort::from(query.sort.unwrap_or(SearchSortDto::Relevance));
    let (before, after, offset) = match sort {
        SearchSort::Time => (
            decode_cursor(query.before, request_id.clone())?,
            decode_cursor(query.after, request_id.clone())?,
            0,
        ),
        SearchSort::Relevance => {
            if query.after.is_some() {
                return Err(ApiError::malformed(request_id));
            }
            let offset = query
                .before
                .map(|value| crate::interface::cursor::decode_offset(&value))
                .transpose()
                .map_err(|_| ApiError::malformed(request_id.clone()))?
                .unwrap_or(0);
            (None, None, offset)
        }
    };
    let result = SearchMessagesQuery {
        text: query.q,
        filters,
        before,
        after,
        page_size,
        sort,
        offset,
    };
    result
        .validate()
        .map_err(|error| ApiError::from_application(error.into(), request_id))?;
    Ok(result)
}

pub(in crate::interface::rest) fn filters(
    chat_id: Option<i64>,
    sender_id: Option<i64>,
    post_author: Option<String>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    include_deleted: Option<bool>,
    request_id: String,
) -> Result<MessageFilters, ApiError> {
    let chat_id = chat_id
        .map(ChatId::from_marked)
        .transpose()
        .map_err(|_| ApiError::malformed(request_id.clone()))?;
    let sender_id = sender_id
        .map(SenderId::from_marked)
        .transpose()
        .map_err(|_| ApiError::malformed(request_id.clone()))?;
    let time_range = TimeRange::new(from, to)
        .map_err(|error| ApiError::from_application(error.into(), request_id))?;
    Ok(MessageFilters {
        chat_id,
        sender_id,
        post_author: post_author.filter(|author| !author.is_empty()),
        time_range,
        include_deleted: include_deleted.unwrap_or(false),
        exclude_bots: false,
    })
}

pub(in crate::interface::rest) fn decode_cursor(
    value: Option<String>,
    request_id: String,
) -> Result<Option<crate::application::MessageCursor>, ApiError> {
    value
        .map(|value| {
            crate::interface::cursor::decode(&value).map_err(|_| ApiError::malformed(request_id))
        })
        .transpose()
}

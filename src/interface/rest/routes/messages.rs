use axum::{Extension, Json, extract::State};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    application::{MessageContextQuery, PageSize, SenderQuery},
    domain::{ChatId, MessageId, SenderId},
};

use super::{
    ApiError, RequestId, RestState,
    dto::{
        MessageContextDto, MessageDto, MessagePageDto, SenderDetailDto, SenderPageDto,
        SenderSortDto, SenderSummaryDto,
    },
    extract::{ApiPath, ApiQuery},
};
use super::{MessageQuery, SearchQuery, build_list_query, build_search_query};

#[utoipa::path(get, path = "/api/v1/messages", params(MessageQuery), responses((status = 200, body = MessagePageDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn list_messages(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<MessageQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MessagePageDto>, ApiError> {
    let query = build_list_query(query, id.0.clone())?;
    let page = state
        .application
        .list_messages(query)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    MessagePageDto::try_from(page)
        .map(Json)
        .map_err(|_| ApiError::internal(id.0))
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/messages", params(("chat_id" = i64, Path), MessageQuery), responses((status = 200, body = MessagePageDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn list_chat_messages(
    State(state): State<RestState>,
    ApiPath((raw_chat_id,)): ApiPath<(i64,)>,
    ApiQuery(mut query): ApiQuery<MessageQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MessagePageDto>, ApiError> {
    let chat_id =
        ChatId::from_marked(raw_chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    if query.chat_id.is_some_and(|filter| filter != chat_id.get()) {
        return Err(ApiError::malformed(id.0));
    }
    query.chat_id = Some(chat_id.get());
    let query = build_list_query(query, id.0.clone())?;
    let page = state
        .application
        .list_messages(query)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    MessagePageDto::try_from(page)
        .map(Json)
        .map_err(|_| ApiError::internal(id.0))
}

#[utoipa::path(get, path = "/api/v1/messages/search", params(SearchQuery), responses((status = 200, body = MessagePageDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn search_messages(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<SearchQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MessagePageDto>, ApiError> {
    let query = build_search_query(query, id.0.clone())?;
    let page = state
        .application
        .search_messages(query)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    MessagePageDto::try_from(page)
        .map(Json)
        .map_err(|_| ApiError::internal(id.0))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct GetMessageQuery {
    /// Also return the message if it was deleted.
    include_deleted: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct ContextQuery {
    /// Messages older than the anchor (0-100, default 20).
    before: Option<u16>,
    /// Messages newer than the anchor (0-100, default 20).
    after: Option<u16>,
    include_deleted: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct SendersQuery {
    /// Maximum senders returned (1-1000, default 100).
    limit: Option<u16>,
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/messages/{message_id}", params(("chat_id" = i64, Path), ("message_id" = i64, Path), GetMessageQuery), responses((status = 200, body = MessageDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn get_message(
    State(state): State<RestState>,
    ApiPath((raw_chat_id, raw_message_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<GetMessageQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MessageDto>, ApiError> {
    let chat_id =
        ChatId::from_marked(raw_chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let message_id =
        MessageId::new(raw_message_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    state
        .application
        .get_message(chat_id, message_id, query.include_deleted.unwrap_or(false))
        .await
        .map(|message| Json(message.into()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/messages/{message_id}/context", params(("chat_id" = i64, Path), ("message_id" = i64, Path), ContextQuery), responses((status = 200, body = MessageContextDto), (status = 400, body = super::ErrorEnvelope), (status = 404, description = "Anchor message not found (or deleted without include_deleted)", body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn message_context(
    State(state): State<RestState>,
    ApiPath((raw_chat_id, raw_message_id)): ApiPath<(i64, i64)>,
    ApiQuery(query): ApiQuery<ContextQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MessageContextDto>, ApiError> {
    let chat_id =
        ChatId::from_marked(raw_chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let message_id =
        MessageId::new(raw_message_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let context = state
        .application
        .message_context(MessageContextQuery {
            chat_id,
            message_id,
            before: query.before.unwrap_or(20),
            after: query.after.unwrap_or(20),
            include_deleted: query.include_deleted.unwrap_or(false),
        })
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    MessageContextDto::try_from(context)
        .map(Json)
        .map_err(|_| ApiError::internal(id.0))
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/senders", params(("chat_id" = i64, Path), SendersQuery), responses((status = 200, description = "Senders with non-deleted messages in the chat, most messages first", body = [SenderSummaryDto]), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn list_chat_senders(
    State(state): State<RestState>,
    ApiPath((raw_chat_id,)): ApiPath<(i64,)>,
    ApiQuery(query): ApiQuery<SendersQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Vec<SenderSummaryDto>>, ApiError> {
    let chat_id =
        ChatId::from_marked(raw_chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let limit = PageSize::new(query.limit.unwrap_or(PageSize::DEFAULT.get()))
        .map_err(|error| ApiError::from_application(error.into(), id.0.clone()))?;
    state
        .application
        .chat_senders(chat_id, limit)
        .await
        .map(|senders| Json(senders.into_iter().map(Into::into).collect()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct SenderListQuery {
    /// Substring of display name or username (NFKC + case-insensitive, Chinese substrings work),
    /// `@username` (exact) or a numeric sender ID.
    q: Option<String>,
    /// Order; default `messages`.
    sort: Option<SenderSortDto>,
    /// Page size (1-1000, default 100).
    limit: Option<u16>,
    /// Cursor from the previous page's `next_cursor`.
    cursor: Option<String>,
    /// Count deleted messages too.
    include_deleted: Option<bool>,
    /// `true` = only bots, `false` = only non-bots (unknown included); omitted = everyone.
    is_bot: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct SenderGetQuery {
    /// Count deleted messages too.
    include_deleted: Option<bool>,
}

#[utoipa::path(get, path = "/api/v1/senders", params(SenderListQuery), responses((status = 200, description = "Senders with message counts, chat counts and activity range", body = SenderPageDto), (status = 400, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn list_senders(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<SenderListQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<SenderPageDto>, ApiError> {
    let limit = PageSize::new(query.limit.unwrap_or(PageSize::DEFAULT.get()))
        .map_err(|error| ApiError::from_application(error.into(), id.0.clone()))?;
    let offset = query
        .cursor
        .map(|value| crate::interface::cursor::decode_offset(&value))
        .transpose()
        .map_err(|_| ApiError::malformed(id.0.clone()))?
        .unwrap_or(0);
    let page = state
        .application
        .search_senders(SenderQuery {
            text: query.q,
            sort: query.sort.map_or_else(Default::default, Into::into),
            limit,
            offset,
            include_deleted: query.include_deleted.unwrap_or(false),
            is_bot: query.is_bot,
        })
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    SenderPageDto::try_from(page)
        .map(Json)
        .map_err(|_| ApiError::internal(id.0))
}

#[utoipa::path(get, path = "/api/v1/senders/{sender_id}", params(("sender_id" = i64, Path, description = "Marked sender ID"), SenderGetQuery), responses((status = 200, description = "Sender with per-chat breakdown", body = SenderDetailDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn get_sender(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    ApiQuery(query): ApiQuery<SenderGetQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<SenderDetailDto>, ApiError> {
    let sender_id = SenderId::from_marked(raw_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    state
        .application
        .sender_detail(sender_id, query.include_deleted.unwrap_or(false))
        .await
        .map(|detail| Json(detail.into()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

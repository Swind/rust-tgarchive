use axum::{Extension, Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    application::{ListMessagesQuery, MessageFilters, PageSize, SearchMessagesQuery, TimeRange},
    domain::{ChatId, MessageId, SenderId},
};

use super::{
    ApiError, RequestId, RestState,
    dto::{ChatDto, HealthDto, MessageDto, MessagePageDto, StatusDto},
    extract::{ApiPath, ApiQuery},
};

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct MessageQuery {
    chat_id: Option<i64>,
    sender_id: Option<i64>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    before: Option<String>,
    after: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct SearchQuery {
    q: String,
    chat_id: Option<i64>,
    sender_id: Option<i64>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    before: Option<String>,
    after: Option<String>,
    limit: Option<u16>,
}

#[utoipa::path(get, path = "/api/v1/status", responses((status = 200, body = StatusDto), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn status(
    State(state): State<RestState>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<StatusDto>, ApiError> {
    state
        .application
        .sync_status()
        .await
        .map(StatusDto::from)
        .map(Json)
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(get, path = "/api/v1/chats", responses((status = 200, body = [ChatDto]), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn list_chats(
    State(state): State<RestState>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Vec<ChatDto>>, ApiError> {
    state
        .application
        .list_chats()
        .await
        .map(|chats| Json(chats.into_iter().map(Into::into).collect()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}", params(("chat_id" = i64, Path, description = "Marked Telegram chat ID")), responses((status = 200, body = ChatDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope)))]
pub(super) async fn get_chat(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<ChatDto>, ApiError> {
    let chat_id = ChatId::from_marked(raw_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    state
        .application
        .get_chat(chat_id)
        .await
        .map(|chat| Json(chat.into()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(get, path = "/api/v1/messages", params(MessageQuery), responses((status = 200, body = MessagePageDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn list_messages(
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
pub(super) async fn list_chat_messages(
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
pub(super) async fn search_messages(
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

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/messages/{message_id}", params(("chat_id" = i64, Path), ("message_id" = i64, Path)), responses((status = 200, body = MessageDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn get_message(
    State(state): State<RestState>,
    ApiPath((raw_chat_id, raw_message_id)): ApiPath<(i64, i64)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MessageDto>, ApiError> {
    let chat_id =
        ChatId::from_marked(raw_chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let message_id =
        MessageId::new(raw_message_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    state
        .application
        .get_message(chat_id, message_id)
        .await
        .map(|message| Json(message.into()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(get, path = "/health/live", responses((status = 200, body = HealthDto)))]
pub(super) async fn live() -> Json<HealthDto> {
    Json(HealthDto {
        status: "live".into(),
    })
}

#[utoipa::path(get, path = "/health/ready", responses((status = 200, body = HealthDto), (status = 503, body = HealthDto)))]
pub(super) async fn ready(State(state): State<RestState>) -> (StatusCode, Json<HealthDto>) {
    let ready = state.application.sync_status().await.is_ok_and(|status| {
        !matches!(
            status.collector.state,
            crate::application::services::ComponentState::Unavailable
                | crate::application::services::ComponentState::Degraded
        )
    });
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(HealthDto {
            status: if ready { "ready" } else { "not_ready" }.into(),
        }),
    )
}

pub(super) async fn not_found(Extension(id): Extension<RequestId>) -> ApiError {
    ApiError::not_found(id.0)
}

fn build_list_query(
    query: MessageQuery,
    request_id: String,
) -> Result<ListMessagesQuery, ApiError> {
    let page_size = PageSize::new(query.limit.unwrap_or(PageSize::DEFAULT.get()))
        .map_err(|error| ApiError::from_application(error.into(), request_id.clone()))?;
    let filters = filters(
        query.chat_id,
        query.sender_id,
        query.from,
        query.to,
        request_id.clone(),
    )?;
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

fn build_search_query(
    query: SearchQuery,
    request_id: String,
) -> Result<SearchMessagesQuery, ApiError> {
    let page_size = PageSize::new(query.limit.unwrap_or(PageSize::DEFAULT.get()))
        .map_err(|error| ApiError::from_application(error.into(), request_id.clone()))?;
    let filters = filters(
        query.chat_id,
        query.sender_id,
        query.from,
        query.to,
        request_id.clone(),
    )?;
    let result = SearchMessagesQuery {
        text: query.q,
        filters,
        before: decode_cursor(query.before, request_id.clone())?,
        after: decode_cursor(query.after, request_id.clone())?,
        page_size,
    };
    result
        .validate()
        .map_err(|error| ApiError::from_application(error.into(), request_id))?;
    Ok(result)
}

fn filters(
    chat_id: Option<i64>,
    sender_id: Option<i64>,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
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
        time_range,
    })
}

fn decode_cursor(
    value: Option<String>,
    request_id: String,
) -> Result<Option<crate::application::MessageCursor>, ApiError> {
    value
        .map(|value| {
            crate::interface::cursor::decode(&value).map_err(|_| ApiError::malformed(request_id))
        })
        .transpose()
}

use axum::{Extension, Json, extract::State, http::StatusCode};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    application::{
        ApplicationError, ChatSort, ListMessagesQuery, MessageContextQuery, MessageFilters,
        PageSize, SearchMessagesQuery, SyncScope, TimeRange,
    },
    domain::{ChatId, MessageId, SenderId},
};

use super::{
    ApiError, RequestId, RestState,
    dto::{
        ChatDto, ChatSortDto, HealthDto, MessageContextDto, MessageDto, MessagePageDto,
        SenderSummaryDto, StatusDto, SyncJobDto, TrackChatDto,
    },
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
    /// Also return deleted messages (flagged with `is_deleted`/`deleted_at`).
    include_deleted: Option<bool>,
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
    /// Also return deleted messages (flagged with `is_deleted`/`deleted_at`).
    include_deleted: Option<bool>,
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

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ChatListQuery {
    /// Only return tracked (collected) chats.
    tracked: Option<bool>,
    /// Sort order; default is by chat ID.
    sort: Option<ChatSortDto>,
}

#[utoipa::path(get, path = "/api/v1/chats", params(ChatListQuery), responses((status = 200, body = [ChatDto]), (status = 400, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn list_chats(
    State(state): State<RestState>,
    ApiQuery(query): ApiQuery<ChatListQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Vec<ChatDto>>, ApiError> {
    state
        .application
        .list_chat_summaries(
            query.tracked.unwrap_or(false),
            query.sort.map_or(ChatSort::Default, Into::into),
        )
        .await
        .map(|chats| Json(chats.into_iter().map(Into::into).collect()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(post, path = "/api/v1/chats/refresh", responses((status = 200, body = [ChatDto]), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn refresh_chats(
    State(state): State<RestState>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Vec<ChatDto>>, ApiError> {
    state
        .application
        .refresh_chats()
        .await
        .map(|chats| Json(chats.into_iter().map(Into::into).collect()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(post, path = "/api/v1/sync", responses((status = 202, body = SyncJobDto), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn sync_all(
    State(state): State<RestState>,
    Extension(id): Extension<RequestId>,
) -> Result<(StatusCode, Json<SyncJobDto>), ApiError> {
    let coordinator = state
        .sync
        .ok_or_else(|| ApiError::from_application(ApplicationError::Busy, id.0.clone()))?;
    coordinator
        .submit(SyncScope::All)
        .await
        .map(|job| (StatusCode::ACCEPTED, Json(job.into())))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(post, path = "/api/v1/chats/{chat_id}/sync", params(("chat_id" = i64, Path)), responses((status = 202, body = SyncJobDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 409, description = "Chat is not tracked (code chat_not_tracked) or a conflicting job is active (code conflict)", body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn sync_chat(
    State(state): State<RestState>,
    ApiPath((raw_chat_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<(StatusCode, Json<SyncJobDto>), ApiError> {
    let chat_id =
        ChatId::from_marked(raw_chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let coordinator = state
        .sync
        .ok_or_else(|| ApiError::from_application(ApplicationError::Busy, id.0.clone()))?;
    coordinator
        .submit(SyncScope::Chat(chat_id))
        .await
        .map(|job| (StatusCode::ACCEPTED, Json(job.into())))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(get, path = "/api/v1/sync/status", responses((status = 200, body = StatusDto), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn sync_status(
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

#[utoipa::path(get, path = "/api/v1/sync/jobs/{job_id}", params(("job_id" = String, Path)), responses((status = 200, body = SyncJobDto), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn get_sync_job(
    State(state): State<RestState>,
    ApiPath((job_id,)): ApiPath<(String,)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<SyncJobDto>, ApiError> {
    let coordinator = state
        .sync
        .ok_or_else(|| ApiError::from_application(ApplicationError::Busy, id.0.clone()))?;
    coordinator
        .get_job(&job_id)
        .await
        .map(|job| Json(job.into()))
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
        .get_chat_summary(chat_id)
        .await
        .map(|chat| Json(chat.into()))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct TrackQuery {
    /// Also enqueue a history sync for the chat (needs the running collector). Reuses an active job instead of duplicating it.
    backfill: Option<bool>,
}

#[utoipa::path(put, path = "/api/v1/chats/{chat_id}/tracking", params(("chat_id" = i64, Path, description = "Marked Telegram chat ID"), TrackQuery), responses((status = 200, description = "Chat is now tracked (idempotent); with backfill=true also carries the history sync job id", body = TrackChatDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, description = "Storage unavailable, or backfill requested without a running collector / with a full queue", body = super::ErrorEnvelope)))]
pub(super) async fn track_chat(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    ApiQuery(query): ApiQuery<TrackQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<TrackChatDto>, ApiError> {
    let chat_id = ChatId::from_marked(raw_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let coordinator = if query.backfill.unwrap_or(false) {
        Some(
            state
                .sync
                .clone()
                .ok_or_else(|| ApiError::from_application(ApplicationError::Busy, id.0.clone()))?,
        )
    } else {
        None
    };
    state
        .application
        .track_chat(chat_id)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    let chat = state
        .application
        .get_chat_summary(chat_id)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    let mut response = TrackChatDto {
        chat: chat.into(),
        backfill_job_id: None,
        backfill: None,
    };
    if let Some(coordinator) = coordinator {
        match coordinator.submit(SyncScope::Chat(chat_id)).await {
            Ok(job) => {
                response.backfill_job_id = Some(job.id);
                response.backfill = Some("queued".into());
            }
            Err(ApplicationError::Conflict) => {
                let active = state
                    .application
                    .sync_status()
                    .await
                    .map_err(|error| ApiError::from_application(error, id.0.clone()))?
                    .sync_jobs
                    .into_iter()
                    .find(|job| {
                        matches!(
                            job.state,
                            crate::application::SyncJobState::Queued
                                | crate::application::SyncJobState::Running
                        ) && (job.scope == SyncScope::All || job.scope == SyncScope::Chat(chat_id))
                    });
                response.backfill_job_id = active.map(|job| job.id);
                response.backfill = Some("already_running".into());
            }
            Err(error) => return Err(ApiError::from_application(error, id.0)),
        }
    }
    Ok(Json(response))
}

#[utoipa::path(delete, path = "/api/v1/chats/{chat_id}/tracking", params(("chat_id" = i64, Path, description = "Marked Telegram chat ID")), responses((status = 200, description = "Chat is no longer tracked; stored messages are kept (idempotent)", body = ChatDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn untrack_chat(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<ChatDto>, ApiError> {
    let chat_id = ChatId::from_marked(raw_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    state
        .application
        .untrack_chat(chat_id)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    state
        .application
        .get_chat_summary(chat_id)
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

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct GetMessageQuery {
    /// Also return the message if it was deleted.
    include_deleted: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct ContextQuery {
    /// Messages older than the anchor (0-100, default 20).
    before: Option<u16>,
    /// Messages newer than the anchor (0-100, default 20).
    after: Option<u16>,
    include_deleted: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(super) struct SendersQuery {
    /// Maximum senders returned (1-1000, default 100).
    limit: Option<u16>,
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/messages/{message_id}", params(("chat_id" = i64, Path), ("message_id" = i64, Path), GetMessageQuery), responses((status = 200, body = MessageDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(super) async fn get_message(
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
pub(super) async fn message_context(
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
pub(super) async fn list_chat_senders(
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
                | crate::application::services::ComponentState::Failed
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
        query.include_deleted,
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
        query.include_deleted,
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
        time_range,
        include_deleted: include_deleted.unwrap_or(false),
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

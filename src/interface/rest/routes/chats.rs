use axum::{Extension, Json, extract::State, http::StatusCode};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{
    application::{ApplicationError, ChatSort, SyncScope},
    domain::ChatId,
};

use super::{
    ApiError, RequestId, RestState,
    dto::{ChatDto, ChatSortDto, StatusDto, SyncJobDto, TrackChatDto},
    extract::{ApiPath, ApiQuery},
};

#[utoipa::path(get, path = "/api/v1/status", responses((status = 200, body = StatusDto), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn status(
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
pub(in crate::interface::rest) struct ChatListQuery {
    /// Only return tracked (collected) chats.
    tracked: Option<bool>,
    /// Sort order; default is by chat ID.
    sort: Option<ChatSortDto>,
}

#[utoipa::path(get, path = "/api/v1/chats", params(ChatListQuery), responses((status = 200, body = [ChatDto]), (status = 400, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn list_chats(
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
pub(in crate::interface::rest) async fn refresh_chats(
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
pub(in crate::interface::rest) async fn sync_all(
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

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(in crate::interface::rest) struct SyncChatQuery {
    /// Re-download the whole history and backfill NULL sender/post-author/forward/attachment
    /// metadata of archived messages (text, versions and deletions are untouched). Resumable.
    refetch: Option<bool>,
}

#[utoipa::path(post, path = "/api/v1/chats/{chat_id}/sync", params(("chat_id" = i64, Path), SyncChatQuery), responses((status = 202, body = SyncJobDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 409, description = "Chat is not tracked (code chat_not_tracked) or a conflicting job is active (code conflict)", body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn sync_chat(
    State(state): State<RestState>,
    ApiPath((raw_chat_id,)): ApiPath<(i64,)>,
    ApiQuery(query): ApiQuery<SyncChatQuery>,
    Extension(id): Extension<RequestId>,
) -> Result<(StatusCode, Json<SyncJobDto>), ApiError> {
    let chat_id =
        ChatId::from_marked(raw_chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let coordinator = state
        .sync
        .ok_or_else(|| ApiError::from_application(ApplicationError::Busy, id.0.clone()))?;
    coordinator
        .submit_with(SyncScope::Chat(chat_id), query.refetch.unwrap_or(false))
        .await
        .map(|job| (StatusCode::ACCEPTED, Json(job.into())))
        .map_err(|error| ApiError::from_application(error, id.0))
}

#[utoipa::path(get, path = "/api/v1/sync/status", responses((status = 200, body = StatusDto), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn sync_status(
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

#[utoipa::path(get, path = "/api/v1/sync/jobs/{job_id}", params(("job_id" = String, Path)), responses((status = 200, description = "Job with per-chat progress", body = SyncJobDto), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn get_sync_job(
    State(state): State<RestState>,
    ApiPath((job_id,)): ApiPath<(String,)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<SyncJobDto>, ApiError> {
    let (job, progress) = state
        .application
        .sync_job_detail(&job_id)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    let titles = state
        .application
        .list_chat_summaries(false, ChatSort::Default)
        .await
        .map_err(|error| ApiError::from_application(error, id.0))?
        .into_iter()
        .filter_map(|c| c.chat.title.map(|t| (c.chat.id.get(), t)))
        .collect();
    Ok(Json(SyncJobDto::from(job).with_chats(progress, &titles)))
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}", params(("chat_id" = i64, Path, description = "Marked Telegram chat ID")), responses((status = 200, body = ChatDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn get_chat(
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
pub(in crate::interface::rest) struct TrackQuery {
    /// Also enqueue a history sync for the chat (needs the running collector). Reuses an active job instead of duplicating it.
    backfill: Option<bool>,
}

#[utoipa::path(put, path = "/api/v1/chats/{chat_id}/tracking", params(("chat_id" = i64, Path, description = "Marked Telegram chat ID"), TrackQuery), responses((status = 200, description = "Chat is now tracked (idempotent); with backfill=true also carries the history sync job id", body = TrackChatDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, description = "Storage unavailable, or backfill requested without a running collector / with a full queue", body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn track_chat(
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
pub(in crate::interface::rest) async fn untrack_chat(
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

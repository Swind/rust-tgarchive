use axum::{Extension, Json, extract::State, http::StatusCode};

use super::{
    ApiError, RequestId, RestState,
    dto::{SetTopicCollectedDto, TopicDto},
    extract::{ApiJson, ApiPath},
};
use crate::domain::{ChatId, MessageId};

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/topics",
    params(("chat_id" = i64, Path, description = "Marked Telegram chat ID")),
    responses(
        (status = 200, description = "Archived topic list and collection settings. Available in query-only mode; new topics are collected by default.", body = [TopicDto]),
        (status = 400, body = super::ErrorEnvelope),
        (status = 404, body = super::ErrorEnvelope),
        (status = 503, body = super::ErrorEnvelope)
    )
)]
pub(in crate::interface::rest) async fn list_chat_topics(
    State(state): State<RestState>,
    ApiPath((chat_id,)): ApiPath<(ChatId,)>,
    Extension(request): Extension<RequestId>,
) -> Result<Json<Vec<TopicDto>>, ApiError> {
    state
        .application
        .list_chat_topics(chat_id)
        .await
        .map(|topics| Json(topics.into_iter().map(Into::into).collect()))
        .map_err(|error| ApiError::from_application(error, request.0))
}

#[utoipa::path(put, path = "/api/v1/chats/{chat_id}/topics/{topic_id}",
    params(("chat_id" = i64, Path, description = "Marked Telegram chat ID"), ("topic_id" = i32, Path, minimum = 1)),
    request_body = SetTopicCollectedDto,
    responses(
        (status = 204, description = "Collection setting saved. Excluding keeps archived messages; re-enabling schedules a history backfill."),
        (status = 400, body = super::ErrorEnvelope),
        (status = 404, description = "Unknown chat or topic", body = super::ErrorEnvelope),
        (status = 409, description = "Chat is not tracked", body = super::ErrorEnvelope),
        (status = 503, description = "Query-only mode or storage unavailable", body = super::ErrorEnvelope)
    )
)]
pub(in crate::interface::rest) async fn set_topic_collected(
    State(state): State<RestState>,
    ApiPath((chat_id, topic_id)): ApiPath<(ChatId, MessageId)>,
    Extension(request): Extension<RequestId>,
    ApiJson(body): ApiJson<SetTopicCollectedDto>,
) -> Result<StatusCode, ApiError> {
    state
        .application
        .set_topic_collected(chat_id, topic_id, body.collected)
        .await
        .map_err(|error| ApiError::from_application(error, request.0))?;
    Ok(StatusCode::NO_CONTENT)
}

use axum::{
    Extension, Json,
    body::Body,
    extract::State,
    http::{StatusCode, header},
};

use crate::domain::ChatId;

use super::{
    ApiError, RequestId, RestState,
    dto::{MediaDownloadDto, MediaPolicyDto, MediaProgressDto},
    extract::{ApiJson, ApiPath},
};
use crate::infrastructure::persistence::sqlite::media::{MediaDownload, MediaPolicy};

fn media_context<'a>(
    state: &'a RestState,
    request_id: &str,
) -> Result<&'a super::MediaContext, ApiError> {
    state
        .media
        .as_ref()
        .ok_or_else(|| ApiError::media_unavailable(request_id.to_owned()))
}

fn media_result<T>(
    result: Result<T, crate::application::RepositoryError>,
    request_id: &str,
) -> Result<T, ApiError> {
    result.map_err(|_| ApiError::storage(request_id.to_owned()))
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/media-policy", params(("chat_id" = i64, Path)), responses((status=200, body=MediaPolicyDto), (status=400, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn get_media_policy(
    State(state): State<RestState>,
    ApiPath((chat_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MediaPolicyDto>, ApiError> {
    let chat = ChatId::from_marked(chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    state
        .application
        .get_chat_summary(chat)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    let media = media_context(&state, &id.0)?;
    media_result(media.store.get_media_policy(chat_id).await, &id.0).map(|value| Json(value.into()))
}

#[utoipa::path(patch, path = "/api/v1/chats/{chat_id}/media-policy", params(("chat_id" = i64, Path)), request_body=MediaPolicyDto, responses((status=200, body=MediaPolicyDto), (status=503, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn set_media_policy(
    State(state): State<RestState>,
    ApiPath((chat_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
    ApiJson(policy): ApiJson<MediaPolicyDto>,
) -> Result<Json<MediaPolicyDto>, ApiError> {
    let chat = ChatId::from_marked(chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    state
        .application
        .get_chat_summary(chat)
        .await
        .map_err(|error| ApiError::from_application(error, id.0.clone()))?;
    let media = media_context(&state, &id.0)?;
    media_result(
        media
            .store
            .set_media_policy(
                chat_id,
                MediaPolicy {
                    auto_archive: policy.auto_archive,
                },
            )
            .await,
        &id.0,
    )?;
    Ok(Json(policy))
}

#[utoipa::path(get, path = "/api/v1/chats/{chat_id}/messages/{message_id}/media", params(("chat_id" = i64, Path), ("message_id" = i64, Path)), responses((status=200, body=[MediaDownloadDto]), (status=503, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn message_media(
    State(state): State<RestState>,
    ApiPath((chat_id, message_id)): ApiPath<(i64, i64)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Vec<MediaDownloadDto>>, ApiError> {
    ChatId::from_marked(chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let media = media_context(&state, &id.0)?;
    media_result(media.store.message_media(chat_id, message_id).await, &id.0)
        .map(|items| Json(items.into_iter().map(Into::into).collect()))
}

#[utoipa::path(post, path = "/api/v1/chats/{chat_id}/media/downloads", params(("chat_id" = i64, Path)), responses((status=202, body=[MediaDownloadDto]), (status=503, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn backfill_media(
    State(state): State<RestState>,
    ApiPath((chat_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<(StatusCode, Json<Vec<MediaDownloadDto>>), ApiError> {
    ChatId::from_marked(chat_id).map_err(|_| ApiError::malformed(id.0.clone()))?;
    let media = media_context(&state, &id.0)?;
    if !media.worker_enabled {
        return Err(ApiError::media_unavailable(id.0));
    }
    let jobs = media_result(media.store.enqueue_media(chat_id, "preview").await, &id.0)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(jobs.into_iter().map(Into::into).collect()),
    ))
}

fn media_id(raw: i64, request_id: &str) -> Result<i64, ApiError> {
    if raw > 0 {
        Ok(raw)
    } else {
        Err(ApiError::malformed(request_id.to_owned()))
    }
}

async fn media_record(
    state: &RestState,
    raw_id: i64,
    request_id: &str,
) -> Result<(super::MediaContext, MediaDownload), ApiError> {
    let id = media_id(raw_id, request_id)?;
    let media = media_context(state, request_id)?.clone();
    let record = media_result(media.store.get_media(id).await, request_id)?
        .ok_or_else(|| ApiError::not_found(request_id.to_owned()))?;
    Ok((media, record))
}

#[utoipa::path(get, path = "/api/v1/media/{media_id}", params(("media_id" = i64, Path)), responses((status=200, body=MediaDownloadDto), (status=404, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn get_media(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<MediaDownloadDto>, ApiError> {
    media_record(&state, raw_id, &id.0)
        .await
        .map(|(_, record)| Json(record.into()))
}

#[utoipa::path(post, path = "/api/v1/media/{media_id}/archive", params(("media_id" = i64, Path)), responses((status=202, body=MediaDownloadDto), (status=200, body=MediaDownloadDto), (status=503, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn archive_media(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<(StatusCode, Json<MediaDownloadDto>), ApiError> {
    let media = media_context(&state, &id.0)?.clone();
    if !media.worker_enabled {
        return Err(ApiError::media_unavailable(id.0));
    }
    let preview = media_id(raw_id, &id.0)?;
    let record = media_result(media.store.request_archive(preview).await, &id.0)?
        .ok_or_else(|| ApiError::not_found(id.0.clone()))?;
    let status = if record.state == "succeeded" {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok((status, Json(record.into())))
}

#[utoipa::path(post, path = "/api/v1/media/{media_id}/retry", params(("media_id" = i64, Path)), responses((status=202, body=MediaDownloadDto), (status=503, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn retry_media(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<(StatusCode, Json<MediaDownloadDto>), ApiError> {
    let media = media_context(&state, &id.0)?.clone();
    if !media.worker_enabled {
        return Err(ApiError::media_unavailable(id.0));
    }
    let record = media_result(
        media.store.retry_media(media_id(raw_id, &id.0)?).await,
        &id.0,
    )?
    .ok_or_else(|| ApiError::not_found(id.0.clone()))?;
    Ok((StatusCode::ACCEPTED, Json(record.into())))
}

#[utoipa::path(get, path = "/api/v1/media/{media_id}/content", params(("media_id" = i64, Path)), responses((status=200, description="Verified local image bytes"), (status=404, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn media_content(
    State(state): State<RestState>,
    ApiPath((raw_id,)): ApiPath<(i64,)>,
    Extension(id): Extension<RequestId>,
) -> Result<axum::response::Response, ApiError> {
    let (context, record) = media_record(&state, raw_id, &id.0).await?;
    if record.state != "succeeded" {
        return Err(ApiError::not_found(id.0));
    }
    let relative = record
        .relative_path
        .as_deref()
        .ok_or_else(|| ApiError::not_found(id.0.clone()))?;
    let relative = std::path::Path::new(relative);
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(ApiError::not_found(id.0));
    }
    let root = tokio::fs::canonicalize(&context.media_dir)
        .await
        .map_err(|_| ApiError::not_found(id.0.clone()))?;
    let path = tokio::fs::canonicalize(root.join(relative))
        .await
        .map_err(|_| ApiError::not_found(id.0.clone()))?;
    if !path.starts_with(&root) {
        return Err(ApiError::not_found(id.0));
    }
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| ApiError::not_found(id.0.clone()))?;
    if !metadata.is_file() || metadata.len() > 20 * 1024 * 1024 {
        return Err(ApiError::not_found(id.0));
    }
    let mime = match record.content_type.as_deref() {
        Some("image/jpeg") => "image/jpeg",
        Some("image/png") => "image/png",
        Some("image/webp") => "image/webp",
        _ => return Err(ApiError::not_found(id.0)),
    };
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| ApiError::not_found(id.0.clone()))?;
    let mut response =
        axum::response::Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, mime.parse().unwrap());
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
    Ok(response)
}

#[utoipa::path(get, path = "/api/v1/media/downloads/status", responses((status=200, body=[MediaProgressDto]), (status=503, body=super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn media_progress(
    State(state): State<RestState>,
    Extension(id): Extension<RequestId>,
) -> Result<Json<Vec<MediaProgressDto>>, ApiError> {
    let media = media_context(&state, &id.0)?;
    media_result(media.store.media_progress().await, &id.0)
        .map(|items| Json(items.into_iter().map(Into::into).collect()))
}

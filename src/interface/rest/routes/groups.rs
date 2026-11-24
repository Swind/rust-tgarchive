use std::collections::HashSet;

use axum::{Extension, Json, extract::State, http::StatusCode};

use super::{
    ApiError, RequestId, RestState,
    dto::{SaveSearchGroupDto, SearchGroupDto},
    extract::{ApiJson, ApiPath},
};
use crate::infrastructure::persistence::search_groups::{
    SearchGroup, SearchGroupError, SearchGroupStore,
};

fn store<'a>(state: &'a RestState, request: &RequestId) -> Result<&'a SearchGroupStore, ApiError> {
    state
        .groups
        .as_ref()
        .ok_or_else(|| ApiError::storage(&request.0))
}

fn api_error(error: SearchGroupError, request: &RequestId) -> ApiError {
    match error {
        SearchGroupError::InvalidInput => ApiError::malformed(&request.0),
        SearchGroupError::NotFound => ApiError::not_found(&request.0),
        SearchGroupError::Storage(_) => ApiError::storage(&request.0),
    }
}

pub(super) async fn load_group(
    state: &RestState,
    id: i64,
    request: &RequestId,
) -> Result<SearchGroup, ApiError> {
    store(state, request)?
        .get(id)
        .await
        .map_err(|e| api_error(e, request))
}

async fn validate_members(
    state: &RestState,
    chat_ids: &[i64],
    request: &RequestId,
) -> Result<(), ApiError> {
    if chat_ids.is_empty() {
        return Ok(());
    }
    let chats = state
        .application
        .list_chats(false)
        .await
        .map_err(|e| ApiError::from_application(e, &request.0))?;
    let mut missing: HashSet<_> = chat_ids.iter().copied().collect();
    for chat in chats {
        missing.remove(&chat.id.get());
        if missing.is_empty() {
            return Ok(());
        }
    }
    Err(ApiError::malformed(&request.0))
}

#[utoipa::path(get, path = "/api/v1/search-groups", responses((status = 200, body = [SearchGroupDto]), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn list_search_groups(
    State(state): State<RestState>,
    Extension(request): Extension<RequestId>,
) -> Result<Json<Vec<SearchGroupDto>>, ApiError> {
    store(&state, &request)?
        .list()
        .await
        .map(|groups| Json(groups.into_iter().map(Into::into).collect()))
        .map_err(|e| api_error(e, &request))
}

#[utoipa::path(get, path = "/api/v1/search-groups/{id}", params(("id" = i64, Path, minimum = 1)), responses((status = 200, body = SearchGroupDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn get_search_group(
    State(state): State<RestState>,
    ApiPath((id,)): ApiPath<(i64,)>,
    Extension(request): Extension<RequestId>,
) -> Result<Json<SearchGroupDto>, ApiError> {
    Ok(Json(load_group(&state, id, &request).await?.into()))
}

#[utoipa::path(post, path = "/api/v1/search-groups", request_body = SaveSearchGroupDto, responses((status = 201, body = SearchGroupDto), (status = 400, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn create_search_group(
    State(state): State<RestState>,
    Extension(request): Extension<RequestId>,
    ApiJson(body): ApiJson<SaveSearchGroupDto>,
) -> Result<(StatusCode, Json<SearchGroupDto>), ApiError> {
    let groups = store(&state, &request)?;
    validate_members(&state, &body.chat_ids, &request).await?;
    let group = groups
        .create(&body.name, body.chat_ids)
        .await
        .map_err(|e| api_error(e, &request))?;
    Ok((StatusCode::CREATED, Json(group.into())))
}

#[utoipa::path(put, path = "/api/v1/search-groups/{id}", params(("id" = i64, Path, minimum = 1)), request_body = SaveSearchGroupDto, responses((status = 200, description = "Name and complete membership replaced", body = SearchGroupDto), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn update_search_group(
    State(state): State<RestState>,
    ApiPath((id,)): ApiPath<(i64,)>,
    Extension(request): Extension<RequestId>,
    ApiJson(body): ApiJson<SaveSearchGroupDto>,
) -> Result<Json<SearchGroupDto>, ApiError> {
    let groups = store(&state, &request)?;
    validate_members(&state, &body.chat_ids, &request).await?;
    let group = groups
        .update(id, &body.name, body.chat_ids)
        .await
        .map_err(|e| api_error(e, &request))?;
    Ok(Json(group.into()))
}

#[utoipa::path(delete, path = "/api/v1/search-groups/{id}", params(("id" = i64, Path, minimum = 1)), responses((status = 204, description = "Group deleted"), (status = 400, body = super::ErrorEnvelope), (status = 404, body = super::ErrorEnvelope), (status = 503, body = super::ErrorEnvelope)))]
pub(in crate::interface::rest) async fn delete_search_group(
    State(state): State<RestState>,
    ApiPath((id,)): ApiPath<(i64,)>,
    Extension(request): Extension<RequestId>,
) -> Result<StatusCode, ApiError> {
    store(&state, &request)?
        .delete(id)
        .await
        .map_err(|e| api_error(e, &request))?;
    Ok(StatusCode::NO_CONTENT)
}

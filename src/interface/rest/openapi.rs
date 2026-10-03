use axum::{Json, http::header, response::IntoResponse};
use thiserror::Error;
use utoipa::OpenApi;

use super::{dto::*, error::*, routes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenApiFormat {
    Json,
    Yaml,
}

#[derive(Debug, Error)]
pub enum SpecError {
    #[error("could not serialize OpenAPI document")]
    Serialization,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        routes::status,
        routes::list_chats,
        routes::refresh_chats,
        routes::sync_all,
        routes::sync_chat,
        routes::sync_status,
        routes::get_sync_job,
        routes::get_chat,
        routes::track_chat,
        routes::untrack_chat,
        routes::list_chat_messages,
        routes::list_messages,
        routes::search_messages,
        routes::get_message,
        routes::live,
        routes::ready,
        json_endpoint,
        yaml_endpoint
    ),
    components(schemas(
        ChatDto,
        ChatKindDto,
        MessageDto,
        AttachmentDto,
        AttachmentKindDto,
        MessagePageDto,
        StatusDto,
        RateLimitDto,
        TrackChatDto,
        ComponentStatusDto,
        SyncJobDto,
        HealthDto,
        ErrorEnvelope,
        ErrorDetail
    )),
    tags((name = "archive", description = "Local Telegram archive queries"))
)]
pub struct ApiDoc;

pub fn openapi_document() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}

pub fn export_openapi(format: OpenApiFormat) -> Result<String, SpecError> {
    match format {
        OpenApiFormat::Json => openapi_document()
            .to_pretty_json()
            .map_err(|_| SpecError::Serialization),
        OpenApiFormat::Yaml => openapi_document()
            .to_yaml()
            .map_err(|_| SpecError::Serialization),
    }
}

#[utoipa::path(get, path = "/openapi.json", responses((status = 200, description = "OpenAPI JSON document", content_type = "application/json")))]
pub(super) async fn json_endpoint() -> impl IntoResponse {
    let document = openapi_document();
    Json(document)
}

#[utoipa::path(get, path = "/openapi.yml", responses((status = 200, description = "OpenAPI YAML document", content_type = "application/yaml")))]
pub(super) async fn yaml_endpoint() -> impl IntoResponse {
    match export_openapi(OpenApiFormat::Yaml) {
        Ok(document) => (
            [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
            document,
        )
            .into_response(),
        Err(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

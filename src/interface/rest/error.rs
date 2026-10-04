use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

use crate::application::ApplicationError;

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ErrorEnvelope {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    pub request_id: String,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    request_id: String,
}

impl ApiError {
    pub(crate) fn malformed(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "malformed_request",
            "Request parameters are invalid",
            request_id,
        )
    }

    pub(crate) fn query_too_large(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::URI_TOO_LONG,
            "query_too_large",
            "Request query exceeds the allowed size",
            request_id,
        )
    }

    pub(crate) fn body_too_large(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "body_too_large",
            "Request body exceeds the allowed size",
            request_id,
        )
    }

    pub(crate) fn not_found(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Resource was not found",
            request_id,
        )
    }

    pub(crate) fn internal(request_id: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "An internal error occurred",
            request_id,
        )
    }

    pub(crate) fn from_application(error: ApplicationError, request_id: impl Into<String>) -> Self {
        let (status, code, message) = match error {
            ApplicationError::Validation(error) => {
                let code = match error {
                    crate::application::ValidationError::InvalidPageSize => "invalid_page_size",
                    crate::application::ValidationError::ConflictingCursors => {
                        "conflicting_cursors"
                    }
                    crate::application::ValidationError::InvalidTimeRange => "invalid_time_range",
                    crate::application::ValidationError::EmptySearch => "empty_search",
                    crate::application::ValidationError::SearchTooLong => "search_too_long",
                    crate::application::ValidationError::InvalidContextSize => {
                        "invalid_context_size"
                    }
                };
                return Self::new(
                    StatusCode::BAD_REQUEST,
                    code,
                    "Request validation failed",
                    request_id,
                );
            }
            ApplicationError::NotFound => {
                (StatusCode::NOT_FOUND, "not_found", "Resource was not found")
            }
            ApplicationError::Conflict => (
                StatusCode::CONFLICT,
                "conflict",
                "Operation conflicts with current state",
            ),
            ApplicationError::NotTracked => (
                StatusCode::CONFLICT,
                "chat_not_tracked",
                "Chat is not tracked; track it first with PUT /api/v1/chats/{chat_id}/tracking",
            ),
            ApplicationError::Busy => (StatusCode::SERVICE_UNAVAILABLE, "busy", "Service is busy"),
            ApplicationError::TelegramUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "telegram_unavailable",
                "Telegram is unavailable",
            ),
            ApplicationError::TelegramFloodWait { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "telegram_rate_limited",
                "Telegram is temporarily rate limited",
            ),
            ApplicationError::RepositoryUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_unavailable",
                "Storage is unavailable",
            ),
            ApplicationError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "An internal error occurred",
            ),
        };
        Self::new(status, code, message, request_id)
    }

    fn new(
        status: StatusCode,
        code: &'static str,
        message: &'static str,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code,
            message,
            request_id: request_id.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorDetail {
                    code: self.code.to_owned(),
                    message: self.message.to_owned(),
                    request_id: self.request_id,
                },
            }),
        )
            .into_response()
    }
}

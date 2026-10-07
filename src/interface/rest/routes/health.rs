use super::{RestState, dto::HealthDto};
use axum::{Json, extract::State, http::StatusCode};

#[utoipa::path(get, path = "/health/live", responses((status = 200, body = HealthDto)))]
pub(in crate::interface::rest) async fn live() -> Json<HealthDto> {
    Json(HealthDto {
        status: "live".into(),
    })
}

#[utoipa::path(get, path = "/health/ready", responses((status = 200, body = HealthDto), (status = 503, body = HealthDto)))]
pub(in crate::interface::rest) async fn ready(
    State(state): State<RestState>,
) -> (StatusCode, Json<HealthDto>) {
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

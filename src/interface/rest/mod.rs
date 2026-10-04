mod dto;
mod error;
mod extract;
mod openapi;
mod routes;
mod web;

use std::{
    net::SocketAddr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use std::time::Duration;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::application::{services::Application, sync::SyncCoordinator};

pub use error::{ApiError, ErrorEnvelope};
pub use openapi::{OpenApiFormat, export_openapi, openapi_document};

const MAX_QUERY_BYTES: usize = 8 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
static PROCESS_NONCE: OnceLock<u128> = OnceLock::new();

#[derive(Clone)]
pub struct RestState {
    pub application: Arc<Application>,
    pub sync: Option<Arc<SyncCoordinator>>,
}

pub fn router(application: Arc<Application>) -> Router {
    router_with_sync(application, None)
}

pub fn router_with_sync(
    application: Arc<Application>,
    sync: Option<Arc<SyncCoordinator>>,
) -> Router {
    let state = RestState { application, sync };
    Router::new()
        .route("/api/v1/status", get(routes::status))
        .route("/api/v1/chats", get(routes::list_chats))
        .route("/api/v1/chats/refresh", axum::routing::post(routes::refresh_chats))
        .route("/api/v1/chats/{chat_id}", get(routes::get_chat))
        .route(
            "/api/v1/chats/{chat_id}/tracking",
            axum::routing::put(routes::track_chat).delete(routes::untrack_chat),
        )
        .route("/api/v1/chats/{chat_id}/messages", get(routes::list_chat_messages))
        .route("/api/v1/chats/{chat_id}/senders", get(routes::list_chat_senders))
        .route(
            "/api/v1/chats/{chat_id}/messages/{message_id}/context",
            get(routes::message_context),
        )
        .route("/api/v1/messages", get(routes::list_messages))
        .route("/api/v1/messages/search", get(routes::search_messages))
        .route(
            "/api/v1/chats/{chat_id}/messages/{message_id}",
            get(routes::get_message),
        )
        .route("/api/v1/sync", axum::routing::post(routes::sync_all))
        .route(
            "/api/v1/chats/{chat_id}/sync",
            axum::routing::post(routes::sync_chat),
        )
        .route("/api/v1/sync/status", get(routes::sync_status))
        .route("/api/v1/sync/jobs/{job_id}", get(routes::get_sync_job))
        .route("/health/live", get(routes::live))
        .route("/health/ready", get(routes::ready))
        .route("/openapi.json", get(openapi::json_endpoint))
        .route("/openapi.yml", get(openapi::yaml_endpoint))
        .fallback(web::fallback)
        .with_state(state)
        .layer(middleware::from_fn(request_limits))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(TraceLayer::new_for_http().make_span_with(|request: &Request<Body>| {
            tracing::info_span!(
                "http.request",
                method = %request.method(),
                path = %request.uri().path(),
                request_id = %request.extensions().get::<RequestId>().map_or("unknown", |id| id.0.as_str())
            )
        }))
        .layer(middleware::from_fn(request_id))
}

/// Environment variable enabling CORS for one local frontend dev origin (default: off).
pub const DEV_CORS_ENV: &str = "TGARCHIVE_DEV_CORS_ORIGIN";

/// Parses a loopback `http` origin such as `http://127.0.0.1:5173`: scheme + host (+ port) only.
pub fn parse_dev_cors_origin(value: &str) -> Result<HeaderValue, String> {
    let invalid = || {
        format!(
            "{DEV_CORS_ENV} must be a loopback http origin like http://127.0.0.1:5173 (got {value:?})"
        )
    };
    let authority = value.strip_prefix("http://").ok_or_else(invalid)?;
    let (host, tail) = match authority.strip_prefix('[') {
        Some(rest) => rest.split_once(']').ok_or_else(invalid)?,
        None => authority.split_at(authority.find(':').unwrap_or(authority.len())),
    };
    if !tail.is_empty()
        && tail
            .strip_prefix(':')
            .is_none_or(|port| port.parse::<u16>().is_err())
    {
        return Err(invalid());
    }
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !loopback {
        return Err(invalid());
    }
    HeaderValue::from_str(value).map_err(|_| invalid())
}

/// Reads [`DEV_CORS_ENV`]; unset or empty disables CORS, anything else must validate.
pub fn dev_cors_from_env() -> Result<Option<CorsLayer>, String> {
    match std::env::var(DEV_CORS_ENV) {
        Ok(value) if !value.is_empty() => Ok(Some(dev_cors_layer(parse_dev_cors_origin(&value)?))),
        _ => Ok(None),
    }
}

pub fn dev_cors_layer(origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |request_origin, _| {
            *request_origin == origin
        }))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
        ])
        .allow_headers([axum::http::header::CONTENT_TYPE, REQUEST_ID])
        .expose_headers([REQUEST_ID])
}

pub fn apply_dev_cors(router: axum::Router, cors: Option<CorsLayer>) -> axum::Router {
    match cors {
        Some(cors) => router.layer(cors),
        None => router,
    }
}

pub fn validate_loopback_bind(address: SocketAddr) -> Result<(), &'static str> {
    if address.ip().is_loopback() {
        Ok(())
    } else {
        Err(
            "REST server may bind only to a loopback address (e.g. 127.0.0.1:8080); the API has no authentication, so use an SSH tunnel or reverse proxy with auth for remote access",
        )
    }
}

async fn request_id(mut request: Request<Body>, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(&REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .map(str::to_owned)
        .unwrap_or_else(new_request_id);

    let request_id_header = HeaderValue::from_str(&request_id).expect("validated request ID");
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    let mut response = next.run(request).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        response = ApiError::body_too_large(request_id.clone()).into_response();
    }
    response.headers_mut().insert(REQUEST_ID, request_id_header);
    response
}

async fn request_limits(request: Request<Body>, next: Next) -> Response {
    if request
        .uri()
        .query()
        .is_some_and(|query| query.len() > MAX_QUERY_BYTES)
    {
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .map_or_else(|| "unknown".to_owned(), |id| id.0.clone());
        return ApiError::query_too_large(request_id).into_response();
    }
    next.run(request).await
}

#[derive(Clone, Debug)]
pub(crate) struct RequestId(pub String);

fn new_request_id() -> String {
    let nonce = *PROCESS_NONCE.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    });
    format!(
        "{nonce:x}-{:x}",
        REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

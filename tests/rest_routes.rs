use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use serde_json::{Value, json};
use tgarchive::{
    application::{
        ArchiveWriter, ChatCheckpoint, ChatRepository, HistoryBoundary, HistoryPage, IngestBatch,
        ListMessagesQuery, MessagePage, MessageRepository, PageSize, RepositoryError,
        SearchMessagesQuery, SyncChatProgress, SyncJob, SyncJobState, SyncRepository,
        TelegramError, TelegramGateway, ingestion_worker,
        services::{Application, CollectorStatusHandle, ComponentState, ComponentStatus},
        sync::{SyncCoordinator, SyncEngine},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageId},
    interface::rest::{
        OpenApiFormat, export_openapi, router, router_with_sync, validate_loopback_bind,
    },
};

#[derive(Clone, Copy)]
enum Failure {
    Unavailable,
    InvalidData,
}

struct FakePorts {
    failure: Option<Failure>,
    jobs: Mutex<HashMap<String, SyncJob>>,
    gateway_gate: Option<Arc<GatewayGate>>,
}

struct GatewayGate {
    entered: Semaphore,
    release: Semaphore,
}

impl FakePorts {
    fn repo_error(&self) -> Option<RepositoryError> {
        self.failure.map(|failure| match failure {
            Failure::Unavailable => RepositoryError::Unavailable("private database detail".into()),
            Failure::InvalidData => RepositoryError::InvalidData("private internal detail".into()),
        })
    }
}

#[async_trait]
impl MessageRepository for FakePorts {
    async fn get(&self, _: ChatId, _: MessageId) -> Result<Option<Message>, RepositoryError> {
        if let Some(error) = self.repo_error() {
            return Err(error);
        }
        Ok(None)
    }
    async fn list(&self, _: ListMessagesQuery) -> Result<MessagePage, RepositoryError> {
        if let Some(error) = self.repo_error() {
            return Err(error);
        }
        Ok(MessagePage {
            items: vec![],
            has_more: false,
            next_cursor: None,
        })
    }
    async fn search(&self, _: SearchMessagesQuery) -> Result<MessagePage, RepositoryError> {
        if let Some(error) = self.repo_error() {
            return Err(error);
        }
        Ok(MessagePage {
            items: vec![],
            has_more: false,
            next_cursor: None,
        })
    }
}

#[async_trait]
impl ChatRepository for FakePorts {
    async fn get(&self, id: ChatId) -> Result<Option<Chat>, RepositoryError> {
        if let Some(error) = self.repo_error() {
            return Err(error);
        }
        Ok(matches!(id.get(), 7 | 9 | 10).then(|| Chat {
            id,
            kind: ChatKind::Private,
            title: Some("fixture".into()),
            username: None,
            tracked: true,
        }))
    }
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
        if let Some(error) = self.repo_error() {
            return Err(error);
        }
        Ok([7, 9, 10]
            .into_iter()
            .map(|id| Chat {
                id: ChatId::from_marked(id).unwrap(),
                kind: ChatKind::Private,
                title: Some("fixture".into()),
                username: None,
                tracked: true,
            })
            .collect())
    }
    async fn save_refresh(&self, _: Vec<Chat>) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn set_tracked(
        &self,
        id: ChatId,
        tracked: bool,
    ) -> Result<Option<Chat>, RepositoryError> {
        if let Some(error) = self.repo_error() {
            return Err(error);
        }
        Ok(matches!(id.get(), 7 | 9 | 10).then(|| Chat {
            id,
            kind: ChatKind::Private,
            title: Some("fixture".into()),
            username: None,
            tracked,
        }))
    }
}

#[async_trait]
impl ArchiveWriter for FakePorts {
    async fn write_batch(&self, _: IngestBatch) -> Result<(), RepositoryError> {
        Ok(())
    }
}

#[async_trait]
impl TelegramGateway for FakePorts {
    async fn list_chats(&self) -> Result<Vec<Chat>, TelegramError> {
        Ok(vec![])
    }
    async fn fetch_history(
        &self,
        _: ChatId,
        _: HistoryBoundary,
        _: PageSize,
    ) -> Result<HistoryPage, TelegramError> {
        if let Some(gate) = &self.gateway_gate {
            gate.entered.add_permits(1);
            let permit = gate
                .release
                .acquire()
                .await
                .expect("test gate remains open");
            permit.forget();
        }
        Ok(HistoryPage {
            chats: vec![],
            senders: vec![],
            records: vec![],
            next_before_message_id: None,
            next_after_message_id: None,
            exhausted: true,
        })
    }
}

#[async_trait]
impl SyncRepository for FakePorts {
    async fn get_checkpoint(&self, _: ChatId) -> Result<Option<ChatCheckpoint>, RepositoryError> {
        Ok(None)
    }
    async fn save_job(&self, job: SyncJob) -> Result<(), RepositoryError> {
        self.jobs.lock().unwrap().insert(job.id.clone(), job);
        Ok(())
    }
    async fn get_job(&self, id: &str) -> Result<Option<SyncJob>, RepositoryError> {
        Ok(self.jobs.lock().unwrap().get(id).cloned())
    }
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        self.repo_error().map_or_else(
            || Ok(self.jobs.lock().unwrap().values().cloned().collect()),
            Err,
        )
    }
    async fn list_chat_progress(&self, _: &str) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        Ok(vec![])
    }
    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

fn app(failure: Option<Failure>) -> Arc<Application> {
    let fake = Arc::new(FakePorts {
        failure,
        jobs: Mutex::new(HashMap::new()),
        gateway_gate: None,
    });
    Application::new(
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake,
        None,
        ComponentStatus {
            state: ComponentState::Disabled,
            detail: None,
        },
    )
    .into()
}

async fn get(
    application: Arc<Application>,
    uri: &str,
    request_id: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().uri(uri);
    if let Some(id) = request_id {
        request = request.header("x-request-id", id);
    }
    router(application)
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn query_and_path_rejections_use_the_error_envelope_and_keep_request_id() {
    for (uri, expected_code) in [
        ("/api/v1/messages?limit=bad", "malformed_request"),
        ("/api/v1/messages?limit=0", "invalid_page_size"),
        ("/api/v1/messages?before=bad", "malformed_request"),
        ("/api/v1/chats/0", "malformed_request"),
    ] {
        let response = get(app(None), uri, Some("test-request-17")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response.headers()["x-request-id"], "test-request-17");
        let body = json_body(response).await;
        assert_eq!(body["error"]["code"], expected_code);
        assert_eq!(body["error"]["request_id"], "test-request-17");
    }
}

#[tokio::test]
async fn maps_not_found_unavailable_and_internal_errors_without_leaking_details() {
    let response = get(app(None), "/api/v1/chats/8", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(response).await["error"]["code"], "not_found");

    let response = get(app(Some(Failure::Unavailable)), "/api/v1/chats", None).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = json_body(response).await.to_string();
    assert!(!body.contains("private database detail"));

    let response = get(app(Some(Failure::InvalidData)), "/api/v1/chats", None).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = json_body(response).await.to_string();
    assert!(!body.contains("private internal detail"));
}

#[tokio::test]
async fn unavailable_refresh_is_reported_without_claiming_success() {
    let response = router(app(None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats/refresh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "telegram_unavailable"
    );
}

#[tokio::test]
async fn sync_routes_are_present_but_do_not_accept_jobs_without_a_coordinator() {
    let response = router(app(None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats/7/sync")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(response).await["error"]["code"], "busy");
}

#[tokio::test]
async fn sync_routes_share_the_coordinator_and_report_duplicates_unknown_chats_and_full_queue() {
    let gate = Arc::new(GatewayGate {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let fake = Arc::new(FakePorts {
        failure: None,
        jobs: Mutex::new(HashMap::new()),
        gateway_gate: Some(gate.clone()),
    });
    let application: Arc<Application> = Application::new(
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        None,
        ComponentStatus {
            state: ComponentState::Disabled,
            detail: None,
        },
    )
    .into();
    let (sink, writer) = ingestion_worker::spawn(fake.clone(), 1);
    let engine = Arc::new(SyncEngine::new(
        fake.clone(),
        fake.clone(),
        fake.clone(),
        sink.clone(),
        PageSize::new(50).unwrap(),
    ));
    let coordinator = SyncCoordinator::spawn(engine, fake.clone(), 1);
    let service = router_with_sync(application, Some(coordinator.clone()));

    let first = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats/7/sync")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = json_body(first).await;
    assert_eq!(first["state"], "queued");
    let first_id = first["id"].as_str().unwrap().to_owned();

    // Keep the first job inside its Telegram call so subsequent submissions
    // exercise the same live coordinator's reservation and bounded queue.
    let permit = gate.entered.acquire().await.unwrap();
    permit.forget();
    let current = service
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/sync/jobs/{first_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::OK);
    assert_eq!(json_body(current).await["state"], "running");

    let duplicate = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats/7/sync")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(duplicate).await["error"]["code"], "conflict");

    let unknown = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats/8/sync")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let queued = service
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats/9/sync")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(queued.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(queued).await["state"], "queued");
    let full = service
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/chats/10/sync")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(full.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(full).await["error"]["code"], "busy");

    gate.release.add_permits(2);
    let completed = coordinator.wait_job(&first_id).await.unwrap();
    assert_eq!(completed.state, SyncJobState::Succeeded);
    coordinator.shutdown().await.unwrap();
    drop(coordinator);
    drop(sink);
    writer.await.unwrap().unwrap();
}

#[tokio::test]
async fn status_health_and_query_route_report_real_application_state() {
    let response = get(app(None), "/api/v1/status", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["collector"]["state"], "disabled");

    assert_eq!(
        get(app(None), "/health/live", None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        get(app(None), "/health/ready", None).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        get(app(Some(Failure::Unavailable)), "/health/ready", None)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    let response = get(app(None), "/api/v1/messages?chat_id=7&limit=50", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        json!({"items": [], "has_more": false, "next_cursor": null})
    );

    let response = get(app(None), "/api/v1/chats/7/messages?limit=20", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["items"], json!([]));

    let response = get(app(None), "/api/v1/messages/search?q=fixture", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["items"], json!([]));
}

#[tokio::test]
async fn openapi_http_endpoints_serve_the_same_generated_document() {
    let response = get(app(None), "/openapi.json", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let json_value = json_body(response).await;

    let response = get(app(None), "/openapi.yml", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/yaml; charset=utf-8"
    );
    let yaml = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    let yaml_value: Value = serde_norway::from_slice(&yaml).unwrap();
    assert_eq!(yaml_value["paths"], json_value["paths"]);
}

#[test]
fn openapi_json_and_yaml_export_without_application_or_database() {
    let json_text = export_openapi(OpenApiFormat::Json).unwrap();
    let json_value: Value = serde_json::from_str(&json_text).unwrap();
    let paths = json_value["paths"].as_object().unwrap();
    for path in [
        "/api/v1/status",
        "/api/v1/chats",
        "/api/v1/chats/refresh",
        "/api/v1/chats/{chat_id}",
        "/api/v1/chats/{chat_id}/messages",
        "/api/v1/messages",
        "/api/v1/messages/search",
        "/api/v1/chats/{chat_id}/messages/{message_id}",
        "/api/v1/sync",
        "/api/v1/chats/{chat_id}/sync",
        "/api/v1/sync/status",
        "/api/v1/sync/jobs/{job_id}",
        "/health/live",
        "/health/ready",
        "/openapi.json",
        "/openapi.yml",
    ] {
        assert!(paths.contains_key(path), "OpenAPI missing {path}");
    }
    let yaml = export_openapi(OpenApiFormat::Yaml).unwrap();
    let yaml_value: Value = serde_norway::from_str(&yaml).unwrap();
    assert_eq!(yaml_value["paths"], json_value["paths"]);
    assert_eq!(yaml_value["components"], json_value["components"]);
}

#[tokio::test]
async fn oversized_query_and_body_keep_the_error_shape() {
    let query = format!("/api/v1/messages?q={}", "x".repeat(8 * 1024));
    let response = get(app(None), &query, None).await;
    assert_eq!(response.status(), StatusCode::URI_TOO_LONG);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "query_too_large"
    );

    let response = router(app(None))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/messages")
                .header("content-length", (16 * 1024 + 1).to_string())
                .body(Body::from(vec![b'x'; 16 * 1024 + 1]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = json_body(response).await;
    assert_eq!(body["error"]["code"], "body_too_large");
    assert!(!body["error"]["request_id"].as_str().unwrap().is_empty());
}

#[test]
fn bind_policy_accepts_only_loopback() {
    assert!(validate_loopback_bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)).is_ok());
    assert!(validate_loopback_bind("0.0.0.0:8080".parse().unwrap()).is_err());
}

#[tokio::test]
async fn status_route_reflects_shared_collector_state_transitions() {
    let fake = Arc::new(FakePorts {
        failure: None,
        jobs: Mutex::new(HashMap::new()),
        gateway_gate: None,
    });
    let handle = CollectorStatusHandle::new(ComponentStatus::new(ComponentState::Starting, None));
    let application: Arc<Application> = Application::new(
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake,
        None,
        handle.clone(),
    )
    .into();

    for (state, detail, expected_ready) in [
        (ComponentState::Starting, None, StatusCode::OK),
        (ComponentState::CatchingUp, None, StatusCode::OK),
        (ComponentState::Running, None, StatusCode::OK),
        (
            ComponentState::Reconnecting,
            Some("attempt 2; next retry in 2000 ms".to_owned()),
            StatusCode::OK,
        ),
        (
            ComponentState::Failed,
            Some("fatal".to_owned()),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (ComponentState::Stopped, None, StatusCode::OK),
    ] {
        handle.set(state, detail.clone());
        let body = json_body(get(application.clone(), "/api/v1/status", None).await).await;
        assert_eq!(body["collector"]["state"], state.as_str());
        assert_eq!(
            body["collector"]["detail"],
            detail.map_or(Value::Null, Value::from)
        );
        assert_eq!(
            get(application.clone(), "/health/ready", None)
                .await
                .status(),
            expected_ready
        );
    }
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn info_logs_never_contain_query_text_or_secrets() {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(buffer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    for uri in [
        "/api/v1/messages/search?q=SECRET-MESSAGE-TEXT",
        "/api/v1/messages?limit=bad&api_hash=SECRET-API-HASH",
    ] {
        get(app(None), uri, Some("log-test-1")).await;
        get(app(Some(Failure::Unavailable)), uri, Some("log-test-2")).await;
    }
    let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    for secret in [
        "SECRET-MESSAGE-TEXT",
        "SECRET-API-HASH",
        "private database detail",
    ] {
        assert!(!logs.contains(secret), "log leaked {secret}: {logs}");
    }
}

#[tokio::test]
async fn status_reports_unresolved_deletions_and_generates_request_ids() {
    let response = get(app(None), "/api/v1/status", None).await;
    let id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(!id.is_empty());
    assert_eq!(json_body(response).await["unresolved_deletions"], 0);
    let other = get(app(None), "/health/live", None).await;
    assert_ne!(other.headers()["x-request-id"].to_str().unwrap(), id);
}

async fn call(service: &axum::Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let response = service
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    (response.status(), json_body(response).await)
}

#[tokio::test]
async fn backfill_enqueues_one_job_reuses_active_ones_and_is_opt_in() {
    let gate = Arc::new(GatewayGate {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let fake = Arc::new(FakePorts {
        failure: None,
        jobs: Mutex::new(HashMap::new()),
        gateway_gate: Some(gate.clone()),
    });
    let application: Arc<Application> = Application::new(
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
        None,
        ComponentStatus::disabled(),
    )
    .into();
    let (sink, writer) = ingestion_worker::spawn(fake.clone(), 1);
    let engine = Arc::new(SyncEngine::new(
        fake.clone(),
        fake.clone(),
        fake.clone(),
        sink.clone(),
        PageSize::new(50).unwrap(),
    ));
    let coordinator = SyncCoordinator::spawn(engine, fake.clone(), 4);
    let service = router_with_sync(application.clone(), Some(coordinator.clone()));

    // Without the flag nothing is enqueued and the response shape is unchanged.
    let (status, body) = call(&service, "PUT", "/api/v1/chats/7/tracking").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.get("backfill_job_id").is_none() && body.get("backfill").is_none());
    assert!(fake.jobs.lock().unwrap().is_empty());
    let (_, body) = call(&service, "PUT", "/api/v1/chats/7/tracking?backfill=false").await;
    assert!(body.get("backfill_job_id").is_none());
    assert!(fake.jobs.lock().unwrap().is_empty());

    let (status, body) = call(&service, "PUT", "/api/v1/chats/7/tracking?backfill=true").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tracked"], true);
    assert_eq!(body["backfill"], "queued");
    let job_id = body["backfill_job_id"].as_str().unwrap().to_owned();
    gate.entered.acquire().await.unwrap().forget();

    // The job is active: a repeated request reuses it instead of duplicating.
    let (status, body) = call(&service, "PUT", "/api/v1/chats/7/tracking?backfill=true").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["backfill"], "already_running");
    assert_eq!(body["backfill_job_id"], job_id);
    assert_eq!(fake.jobs.lock().unwrap().len(), 1);

    // Unknown chats are rejected before anything is enqueued.
    let (status, _) = call(&service, "PUT", "/api/v1/chats/8/tracking?backfill=true").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Without a running collector, backfill is refused up front (503) instead of silently skipped.
    let (status, body) = call(
        &router(application),
        "PUT",
        "/api/v1/chats/7/tracking?backfill=true",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "busy");

    gate.release.add_permits(2);
    coordinator.wait_job(&job_id).await.unwrap();
    coordinator.shutdown().await.unwrap();
    drop(coordinator);
    drop(sink);
    writer.await.unwrap().unwrap();
}

#[tokio::test]
async fn status_exposes_the_history_rate_limit_only_when_a_pacer_is_attached() {
    use tgarchive::application::pacer::RatePacer;
    let fake = Arc::new(FakePorts {
        failure: None,
        jobs: Mutex::new(HashMap::new()),
        gateway_gate: None,
    });
    let build = || {
        Application::new(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            None,
            ComponentStatus::disabled(),
        )
    };
    let body = json_body(get(Arc::new(build()), "/api/v1/status", None).await).await;
    assert!(body.get("rate_limit").is_none());

    let pacer = Arc::new(RatePacer::new(std::time::Duration::from_millis(1000)));
    let application = Arc::new(build().with_rate_limit(pacer.clone()));
    let body = json_body(get(application.clone(), "/api/v1/status", None).await).await;
    assert_eq!(body["rate_limit"]["interval_ms"], 1000);
    assert_eq!(body["rate_limit"]["last_flood_wait_secs"], Value::Null);
    pacer.on_flood(42, false);
    let body = json_body(get(application, "/api/v1/status", None).await).await;
    assert_eq!(body["rate_limit"]["interval_ms"], 2000);
    assert_eq!(body["rate_limit"]["base_interval_ms"], 1000);
    assert_eq!(body["rate_limit"]["last_flood_wait_secs"], 42);
    assert!(body["rate_limit"]["last_flood_at"].is_string());
}

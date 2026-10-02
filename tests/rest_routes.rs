use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use async_trait::async_trait;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use serde_json::{Value, json};
use telegram_message_archive::{
    application::{
        ArchiveWriter, ChatCheckpoint, ChatRepository, HistoryBoundary, HistoryPage, IngestBatch,
        ListMessagesQuery, MessagePage, MessageRepository, PageSize, RepositoryError,
        SearchMessagesQuery, SyncChatProgress, SyncJob, SyncRepository, TelegramError,
        TelegramGateway,
        services::{Application, ComponentState, ComponentStatus},
    },
    domain::{Chat, ChatId, ChatKind, Message, MessageId},
    interface::rest::{OpenApiFormat, export_openapi, router, validate_loopback_bind},
};

#[derive(Clone, Copy)]
enum Failure {
    Unavailable,
    InvalidData,
}

struct FakePorts {
    failure: Option<Failure>,
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
        Ok((id.get() == 7).then(|| Chat {
            id,
            kind: ChatKind::Private,
            title: Some("fixture".into()),
            username: None,
        }))
    }
    async fn list(&self) -> Result<Vec<Chat>, RepositoryError> {
        if let Some(error) = self.repo_error() {
            return Err(error);
        }
        Ok(vec![Chat {
            id: ChatId::from_marked(7).unwrap(),
            kind: ChatKind::Private,
            title: Some("fixture".into()),
            username: None,
        }])
    }
    async fn save_refresh(&self, _: Vec<Chat>) -> Result<(), RepositoryError> {
        Ok(())
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
    async fn save_job(&self, _: SyncJob) -> Result<(), RepositoryError> {
        Ok(())
    }
    async fn get_job(&self, _: &str) -> Result<Option<SyncJob>, RepositoryError> {
        Ok(None)
    }
    async fn list_jobs(&self) -> Result<Vec<SyncJob>, RepositoryError> {
        self.repo_error().map_or(Ok(vec![]), Err)
    }
    async fn list_chat_progress(&self, _: &str) -> Result<Vec<SyncChatProgress>, RepositoryError> {
        Ok(vec![])
    }
    async fn recover_interrupted(&self) -> Result<(), RepositoryError> {
        Ok(())
    }
}

fn app(failure: Option<Failure>) -> Arc<Application> {
    let fake = Arc::new(FakePorts { failure });
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
        "/api/v1/chats/{chat_id}",
        "/api/v1/chats/{chat_id}/messages",
        "/api/v1/messages",
        "/api/v1/messages/search",
        "/api/v1/chats/{chat_id}/messages/{message_id}",
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

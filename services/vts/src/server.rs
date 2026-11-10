use std::sync::Arc;

use http_body_util::{BodyExt, Full, Limited};
use http_server::{HttpServer, error_response, json_response};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tracing::Instrument;

use crate::{registry::Registry, verify::verify_token};

const MAX_BODY_BYTES: usize = 16 * 1024;

pub struct VtsServer {
    _http: HttpServer,
}

pub struct Secrets {
    pub registry: Registry,
    /// The secret every temporary app's tokens are signed with.
    pub temporary_app_secret: Option<String>,
}

impl VtsServer {
    pub fn run(listener: TcpListener, secrets: Arc<Secrets>) -> Self {
        Self {
            _http: HttpServer::run(listener, move |request, _peer| {
                handle(request, secrets.clone())
            }),
        }
    }
}

type JsonResponse = Response<Full<Bytes>>;

#[derive(Deserialize)]
struct VerifyRequest {
    token: String,
}

async fn handle(request: Request<Incoming>, secrets: Arc<Secrets>) -> JsonResponse {
    let (head, body) = request.into_parts();
    let span = tracing::info_span!(
        "vts.request",
        "http.request.method" = %head.method,
        "url.path" = head.uri.path(),
        "http.response.status_code" = tracing::field::Empty,
        app_id = tracing::field::Empty,
        is_relay = tracing::field::Empty,
        outcome = tracing::field::Empty,
    );
    telemetry::continue_trace_from(&span, &head.headers);
    let response = async {
        match (head.method, head.uri.path()) {
            (Method::GET, "/healthz") => json_response(StatusCode::OK, &json!({ "status": "ok" })),
            (Method::POST, "/verify") => verify(body, &secrets).await,
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    response
}

async fn verify(body: Incoming, secrets: &Secrets) -> JsonResponse {
    let request = match Limited::new(body, MAX_BODY_BYTES).collect().await {
        Ok(collected) => serde_json::from_slice::<VerifyRequest>(&collected.to_bytes()).ok(),
        Err(_) => None,
    };
    let Some(request) = request else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let apps = secrets.registry.current().await;
    let span = tracing::Span::current();
    match verify_token(
        &request.token,
        &apps,
        secrets.temporary_app_secret.as_deref(),
    ) {
        Ok(verified) => {
            span.record("app_id", verified.app_id.as_str());
            span.record("is_relay", verified.is_relay);
            span.record("outcome", "verified");
            tracing::info!(app_id = %verified.app_id, is_relay = verified.is_relay, "verify ok");
            json_response(
                StatusCode::OK,
                &json!({
                    "appId": verified.app_id,
                    "isRelay": verified.is_relay,
                    "claims": verified.claims,
                }),
            )
        }
        Err(reason) => {
            span.record("outcome", reason.as_str());
            tracing::info!(reason = reason.as_str(), "verify rejected");
            error_response(StatusCode::UNAUTHORIZED, reason.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use app_registry::testing::TemporaryRegistryFile;
    use serde_json::Value;

    use super::*;
    use crate::test_support::{
        APP_SECRET, TEMPORARY_APP_ID, TEMPORARY_SECRET, app_claims, capture_recorded_fields,
        temporary_app_claims, test_apps_json, token,
    };

    struct TestServer {
        base_url: String,
        _registry_file: TemporaryRegistryFile,
        _server: VtsServer,
    }

    async fn start_server() -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let registry_file = TemporaryRegistryFile::new();
        registry_file.write(&test_apps_json());
        let registry = Registry::load(registry_file.store()).await.unwrap();
        TestServer {
            base_url,
            _registry_file: registry_file,
            _server: VtsServer::run(
                listener,
                Arc::new(Secrets {
                    registry,
                    temporary_app_secret: Some(TEMPORARY_SECRET.into()),
                }),
            ),
        }
    }

    fn url(server: &TestServer, path: &str) -> String {
        format!("{}{path}", server.base_url)
    }

    async fn post_verify(server: &TestServer, body: impl Into<reqwest::Body>) -> reqwest::Response {
        http_client::client()
            .post(url(server, "/verify"))
            .body(body)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn post_verify_returns_200_with_the_claims_for_a_valid_token() {
        // Arrange
        let server = start_server().await;
        let claims = app_claims();
        let body = json!({ "token": token(&claims, APP_SECRET) }).to_string();

        // Act
        let response = post_verify(&server, body).await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "appId": "APP", "isRelay": false, "claims": claims })
        );
    }

    #[tokio::test]
    async fn post_verify_returns_200_for_a_temporary_app_token() {
        // Arrange
        let server = start_server().await;
        let body = json!({ "token": token(&temporary_app_claims(), TEMPORARY_SECRET) }).to_string();

        // Act
        let response = post_verify(&server, body).await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(
            (&body["appId"], &body["isRelay"]),
            (&json!(TEMPORARY_APP_ID), &json!(false))
        );
    }

    #[tokio::test]
    async fn post_verify_returns_401_with_the_reason_for_a_rejected_token() {
        // Arrange
        let server = start_server().await;
        let body = json!({ "token": token(&app_claims(), "wrong") }).to_string();

        // Act
        let response = post_verify(&server, body).await;

        // Assert
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "error": "invalid_signature" })
        );
    }

    #[tokio::test]
    async fn post_verify_records_the_app_on_the_request_span_for_a_valid_token() {
        // Arrange
        let (recorded, _guard) = capture_recorded_fields();
        let server = start_server().await;
        let signed = token(&app_claims(), APP_SECRET);
        let body = json!({ "token": signed }).to_string();

        // Act
        post_verify(&server, body).await;

        // Assert
        let fields = recorded.of("vts.request");
        assert_eq!(fields["app_id"], "APP");
        assert_eq!(fields["is_relay"], "false");
        assert_eq!(fields["outcome"], "verified");
        assert!(fields.values().all(|value| !value.contains(&signed)));
    }

    #[tokio::test]
    async fn post_verify_records_the_reason_on_the_request_span_for_a_rejected_token() {
        // Arrange
        let (recorded, _guard) = capture_recorded_fields();
        let server = start_server().await;
        let body = json!({ "token": token(&app_claims(), "wrong") }).to_string();

        // Act
        post_verify(&server, body).await;

        // Assert
        let fields = recorded.of("vts.request");
        assert_eq!(fields["outcome"], "invalid_signature");
        assert!(!fields.contains_key("app_id"));
        assert!(!fields.contains_key("is_relay"));
    }

    #[tokio::test]
    async fn post_verify_returns_400_when_the_body_has_no_token() {
        // Arrange
        let server = start_server().await;

        // Act
        let response = post_verify(&server, "{}").await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_verify_returns_400_for_a_non_json_body() {
        // Arrange
        let server = start_server().await;

        // Act
        let response = post_verify(&server, "token=abc").await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn post_verify_returns_400_for_a_body_over_the_size_limit() {
        // Arrange
        let server = start_server().await;
        let body = json!({ "token": "a".repeat(MAX_BODY_BYTES) }).to_string();

        // Act
        let response = post_verify(&server, body).await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_healthz_returns_200() {
        // Arrange
        let server = start_server().await;

        // Act
        let response = http_client::client()
            .get(url(&server, "/healthz"))
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_routes_return_404() {
        // Arrange
        let server = start_server().await;

        // Act
        let response = http_client::client()
            .get(url(&server, "/verify"))
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

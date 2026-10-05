use std::{convert::Infallible, sync::Arc};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{CONTENT_TYPE, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{apps::Apps, verify::verify_token};

const MAX_BODY_BYTES: usize = 16 * 1024;

pub struct VtsServer {
    _accept_handle: JoinHandle<()>,
}

impl VtsServer {
    pub fn run(listener: TcpListener, apps: Arc<Apps>) -> Self {
        Self {
            _accept_handle: tokio::spawn(accept_connections(listener, apps)),
        }
    }
}

async fn accept_connections(listener: TcpListener, apps: Arc<Apps>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                continue;
            }
        };
        let apps = apps.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, apps.clone()));
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                tracing::debug!(%peer, %error, "connection closed with error");
            }
        });
    }
}

type JsonResponse = Response<Full<Bytes>>;

#[derive(Deserialize)]
struct VerifyRequest {
    token: String,
}

async fn handle(request: Request<Incoming>, apps: Arc<Apps>) -> Result<JsonResponse, Infallible> {
    let (head, body) = request.into_parts();
    let response = match (head.method, head.uri.path()) {
        (Method::GET, "/healthz") => json_response(StatusCode::OK, json!({ "status": "ok" })),
        (Method::POST, "/verify") => verify(body, &apps).await,
        _ => error_response(StatusCode::NOT_FOUND, "not_found"),
    };
    Ok(response)
}

async fn verify(body: Incoming, apps: &Apps) -> JsonResponse {
    let request = match Limited::new(body, MAX_BODY_BYTES).collect().await {
        Ok(collected) => serde_json::from_slice::<VerifyRequest>(&collected.to_bytes()).ok(),
        Err(_) => None,
    };
    let Some(request) = request else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match verify_token(&request.token, apps) {
        Ok(verified) => {
            tracing::info!(app_id = %verified.app_id, is_relay = verified.is_relay, "verify ok");
            json_response(
                StatusCode::OK,
                json!({
                    "appId": verified.app_id,
                    "isRelay": verified.is_relay,
                    "claims": verified.claims,
                }),
            )
        }
        Err(reason) => {
            tracing::info!(reason = reason.as_str(), "verify rejected");
            error_response(StatusCode::UNAUTHORIZED, reason.as_str())
        }
    }
}

fn error_response(status: StatusCode, error: &str) -> JsonResponse {
    json_response(status, json!({ "error": error }))
}

fn json_response(status: StatusCode, body: Value) -> JsonResponse {
    let mut response = Response::new(Full::from(body.to_string()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{APP_SECRET, app_claims, test_apps, token};

    struct TestServer {
        base_url: String,
        _server: VtsServer,
    }

    async fn start_server() -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        TestServer {
            base_url,
            _server: VtsServer::run(listener, Arc::new(test_apps())),
        }
    }

    fn url(server: &TestServer, path: &str) -> String {
        format!("{}{path}", server.base_url)
    }

    async fn post_verify(server: &TestServer, body: impl Into<reqwest::Body>) -> reqwest::Response {
        reqwest::Client::new()
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
        let response = reqwest::get(url(&server, "/healthz")).await.unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_routes_return_404() {
        // Arrange
        let server = start_server().await;

        // Act
        let response = reqwest::get(url(&server, "/verify")).await.unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

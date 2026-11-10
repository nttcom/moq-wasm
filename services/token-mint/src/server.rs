use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use http_body_util::{BodyExt, Full, Limited};
use http_server::{HttpServer, error_response, json_response};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue,
    },
};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpListener;
use tracing::Instrument;

use crate::{
    issuer::{RefreshError, TokenMint},
    rate_limit::RateLimit,
};

const MAX_BODY_BYTES: usize = 16 * 1024;

pub struct TokenMintServer {
    _http: HttpServer,
}

struct ServerState {
    issuer: TokenMint,
    issue_limit: RateLimit,
    trusted_proxy_hops: usize,
}

impl TokenMintServer {
    pub fn run(
        listener: TcpListener,
        issuer: TokenMint,
        issue_limit: RateLimit,
        trusted_proxy_hops: usize,
    ) -> Self {
        let state = Arc::new(ServerState {
            issuer,
            issue_limit,
            trusted_proxy_hops,
        });
        Self {
            _http: HttpServer::run(listener, move |request, peer| {
                handle(request, peer, state.clone())
            }),
        }
    }
}

type JsonResponse = Response<Full<Bytes>>;

#[derive(Deserialize)]
struct RefreshRequest {
    token: String,
}

async fn handle(
    request: Request<Incoming>,
    peer: SocketAddr,
    state: Arc<ServerState>,
) -> JsonResponse {
    let (head, body) = request.into_parts();
    let span = tracing::info_span!(
        "token_mint.request",
        "http.request.method" = %head.method,
        "url.path" = head.uri.path(),
        "http.response.status_code" = tracing::field::Empty,
        app_id = tracing::field::Empty,
    );
    telemetry::continue_trace_from(&span, &head.headers);
    let response = async {
        match (&head.method, head.uri.path()) {
            (&Method::OPTIONS, _) => empty_response(StatusCode::NO_CONTENT),
            (&Method::GET, "/healthz") => json_response(StatusCode::OK, &json!({ "status": "ok" })),
            (&Method::POST, "/tokens") => {
                let client = client_address(&head.headers, peer, state.trusted_proxy_hops);
                issue(&state, client)
            }
            (&Method::POST, "/tokens/refresh") => refresh(body, &state.issuer).await,
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    with_cors(response)
}

fn issue(state: &ServerState, client: IpAddr) -> JsonResponse {
    if !state.issue_limit.allow(client, Instant::now()) {
        tracing::info!(error = "rate_limited", "temporary token refused");
        return error_response(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    match state.issuer.issue(unix_now()) {
        Ok(issued) => {
            tracing::Span::current().record("app_id", issued.app_id.as_str());
            tracing::info!(app_id = %issued.app_id, "temporary token issued");
            json_response(StatusCode::CREATED, &json!(issued))
        }
        Err(error) => internal_error(&error),
    }
}

async fn refresh(body: Incoming, issuer: &TokenMint) -> JsonResponse {
    let request = match Limited::new(body, MAX_BODY_BYTES).collect().await {
        Ok(collected) => serde_json::from_slice::<RefreshRequest>(&collected.to_bytes()).ok(),
        Err(_) => None,
    };
    let Some(request) = request else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match issuer.refresh(&request.token, unix_now()) {
        Ok(issued) => {
            tracing::Span::current().record("app_id", issued.app_id.as_str());
            tracing::info!(app_id = %issued.app_id, "temporary token refreshed");
            json_response(StatusCode::OK, &json!(issued))
        }
        Err(RefreshError::Internal(error)) => internal_error(&error),
        Err(rejected) => {
            let error = match rejected {
                RefreshError::Expired => "expired_token",
                _ => "invalid_token",
            };
            tracing::info!(error, "temporary token refresh refused");
            error_response(StatusCode::UNAUTHORIZED, error)
        }
    }
}

/// `trusted_proxy_hops` proxies in front of the server append to
/// `X-Forwarded-For`; the client address is the entry the outermost of them
/// appended, or with `0` the connection's peer address.
fn client_address(headers: &HeaderMap, peer: SocketAddr, trusted_proxy_hops: usize) -> IpAddr {
    if trusted_proxy_hops == 0 {
        return peer.ip();
    }
    let entries: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    entries
        .len()
        .checked_sub(trusted_proxy_hops)
        .and_then(|index| entries[index].parse().ok())
        .unwrap_or_else(|| peer.ip())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is after the Unix epoch")
        .as_secs()
}

fn with_cors(mut response: JsonResponse) -> JsonResponse {
    let headers = response.headers_mut();
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST"),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    response
}

fn internal_error(error: &anyhow::Error) -> JsonResponse {
    tracing::error!(
        error = format!("{error:#}"),
        "temporary token signing failed"
    );
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
}

fn empty_response(status: StatusCode) -> JsonResponse {
    let mut response = Response::new(Full::default());
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use auth_token::verify_signature;
    use serde_json::Value;

    use super::*;

    const SECRET: &str = "temporary-secret";

    struct TestServer {
        base_url: String,
        _server: TokenMintServer,
    }

    async fn start_server(max_issues: u32) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        TestServer {
            base_url,
            _server: TokenMintServer::run(
                listener,
                TokenMint::new(SECRET.into()),
                RateLimit::new(max_issues, Duration::from_secs(3600)),
                1,
            ),
        }
    }

    async fn post(
        server: &TestServer,
        path: &str,
        forwarded_for: &str,
        body: impl Into<reqwest::Body>,
    ) -> reqwest::Response {
        http_client::client()
            .post(format!("{}{path}", server.base_url))
            .header("x-forwarded-for", forwarded_for)
            .body(body)
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn post_tokens_returns_201_with_a_signed_token_to_any_origin() {
        // Arrange
        let server = start_server(10).await;

        // Act
        let response = post(&server, "/tokens", "192.0.2.1", "").await;

        // Assert
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        let body = response.json::<Value>().await.unwrap();
        verify_signature(body["token"].as_str().unwrap(), SECRET).unwrap();
        assert!(body["appId"].as_str().unwrap().starts_with("temp-"));
    }

    #[tokio::test]
    async fn post_tokens_returns_429_once_a_client_reaches_the_limit() {
        // Arrange
        let server = start_server(1).await;
        post(&server, "/tokens", "192.0.2.1", "").await;

        // Act
        let response = post(&server, "/tokens", "192.0.2.1", "").await;

        // Assert
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn post_tokens_counts_the_address_the_trusted_proxy_appended() {
        // Arrange
        let server = start_server(1).await;
        post(&server, "/tokens", "198.51.100.7, 192.0.2.1", "").await;

        // Act
        let response = post(&server, "/tokens", "203.0.113.9, 192.0.2.1", "").await;

        // Assert: a client-written first entry does not make it another client
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn post_tokens_refresh_returns_200_with_a_token_for_the_same_app() {
        // Arrange
        let server = start_server(10).await;
        let issued = post(&server, "/tokens", "192.0.2.1", "")
            .await
            .json::<Value>()
            .await
            .unwrap();
        let body = json!({ "token": issued["token"] }).to_string();

        // Act
        let response = post(&server, "/tokens/refresh", "192.0.2.1", body).await;

        // Assert
        assert_eq!(response.status(), StatusCode::OK);
        let refreshed = response.json::<Value>().await.unwrap();
        assert_eq!(refreshed["appId"], issued["appId"]);
    }

    #[tokio::test]
    async fn post_tokens_refresh_returns_401_for_a_token_signed_elsewhere() {
        // Arrange
        let server = start_server(10).await;
        let token = auth_token::sign_token(&json!({ "appId": "temp-x", "exp": u64::MAX }), "other")
            .unwrap();
        let body = json!({ "token": token }).to_string();

        // Act
        let response = post(&server, "/tokens/refresh", "192.0.2.1", body).await;

        // Assert
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "error": "invalid_token" })
        );
    }

    #[tokio::test]
    async fn post_tokens_refresh_returns_400_without_a_token() {
        // Arrange
        let server = start_server(10).await;

        // Act
        let response = post(&server, "/tokens/refresh", "192.0.2.1", "{}").await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn options_answers_the_preflight() {
        // Arrange
        let server = start_server(10).await;

        // Act
        let response = http_client::client()
            .request(
                reqwest::Method::OPTIONS,
                format!("{}/tokens", server.base_url),
            )
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "content-type"
        );
    }
}

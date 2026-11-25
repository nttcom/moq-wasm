use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Instant,
};

use auth_token::unix_now;
use http_server::{
    HttpServer, JsonResponse, allow_any_origin, bearer_token, client_ip, empty_response,
    error_response, healthz_response, json_response, raw_json_response, read_json,
};
use hyper::{Method, Request, StatusCode, body::Incoming, http::request::Parts};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tracing::Instrument;

use crate::{
    bot_proxy::BotProxy,
    issuer::{RefreshError, TemporaryApps},
    rate_limit::RateLimit,
};

pub struct TemporaryAppsServer {
    _http: HttpServer,
}

struct ServerState {
    issuer: TemporaryApps,
    issue_limit: RateLimit,
    trusted_proxy_hops: usize,
    /// `None` answers every `/bots` route 404.
    bots: Option<BotProxy>,
}

impl TemporaryAppsServer {
    pub fn run(
        listener: TcpListener,
        issuer: TemporaryApps,
        issue_limit: RateLimit,
        trusted_proxy_hops: usize,
        bots: Option<BotProxy>,
    ) -> Self {
        let state = Arc::new(ServerState {
            issuer,
            issue_limit,
            trusted_proxy_hops,
            bots,
        });
        Self {
            _http: HttpServer::run(listener, move |request, peer| {
                handle(request, peer, state.clone())
            }),
        }
    }
}

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
        "temporary_apps.request",
        "http.request.method" = %head.method,
        "url.path" = head.uri.path(),
        "http.response.status_code" = tracing::field::Empty,
        app_id = tracing::field::Empty,
    );
    telemetry_otlp::continue_trace_from(&span, &head.headers);
    let response = async {
        match (&head.method, head.uri.path()) {
            (&Method::OPTIONS, _) => empty_response(StatusCode::NO_CONTENT),
            (&Method::GET, "/healthz") => healthz_response(),
            (&Method::POST, "/tokens") => {
                let client = client_ip(&head.headers, peer.ip(), state.trusted_proxy_hops);
                issue(&state, client)
            }
            (&Method::POST, "/tokens/refresh") => refresh(body, &state.issuer).await,
            (_, path) if path.starts_with("/bots/") => bots(&state, &head, peer, body).await,
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    allow_any_origin(response, "GET, POST, DELETE", "content-type, authorization")
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

async fn refresh(body: Incoming, issuer: &TemporaryApps) -> JsonResponse {
    let Some(request) = read_json::<RefreshRequest, _>(body).await else {
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

/// Passes a page's call on to the app server routes of `bot-dispatcher`, with a launch
/// token for the app its own token names. The routes the bots call are 404.
async fn bots(state: &ServerState, head: &Parts, peer: SocketAddr, body: Incoming) -> JsonResponse {
    let Some(proxy) = &state.bots else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let segments: Vec<&str> = head.uri.path().trim_start_matches('/').split('/').collect();
    let bot_type = match (&head.method, segments.as_slice()) {
        (&Method::POST | &Method::GET, ["bots", bot_type])
        | (&Method::GET | &Method::DELETE, ["bots", bot_type, _]) => *bot_type,
        _ => return error_response(StatusCode::NOT_FOUND, "not_found"),
    };
    if !proxy.allows(bot_type) {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    }
    let page_token = bearer_token(&head.headers);
    let app_id = match page_token.map(|token| state.issuer.verify(token, unix_now())) {
        Some(Ok(app_id)) => app_id,
        Some(Err(RefreshError::Internal(error))) => return internal_error(&error),
        _ => {
            tracing::info!(bot_type, error = "invalid_token", "bot request refused");
            return error_response(StatusCode::UNAUTHORIZED, "invalid_token");
        }
    };
    tracing::Span::current().record("app_id", app_id.as_str());
    let body = if head.method == Method::POST {
        let client = client_ip(&head.headers, peer.ip(), state.trusted_proxy_hops);
        if !proxy.allow_launch(client, Instant::now()) {
            tracing::info!(%app_id, bot_type, error = "rate_limited", "bot request refused");
            return error_response(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
        }
        let Some(body) = read_json::<Value, _>(body).await else {
            return error_response(StatusCode::BAD_REQUEST, "invalid_request");
        };
        // Anyone can create a temporary app, so none may keep a bot in an
        // empty room until its longest run.
        if body.get("stay_when_empty") == Some(&Value::Bool(true)) {
            tracing::info!(%app_id, bot_type, error = "forbidden", "bot request refused");
            return error_response(StatusCode::FORBIDDEN, "forbidden");
        }
        Some(body)
    } else {
        None
    };
    let launch_token = match state.issuer.sign_launch(&app_id, unix_now()) {
        Ok(token) => token,
        Err(error) => return internal_error(&error),
    };
    let path_and_query = head
        .uri
        .path_and_query()
        .map_or(head.uri.path(), |path| path.as_str());
    match proxy
        .forward(head.method.clone(), path_and_query, &launch_token, body)
        .await
    {
        Ok((status, body)) => {
            tracing::info!(%app_id, bot_type, "bot request forwarded");
            raw_json_response(status, body)
        }
        Err(error) => {
            tracing::warn!(%app_id, bot_type, error = format!("{error:#}"), "bot-dispatcher unreachable");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "bot_dispatcher_unavailable",
            )
        }
    }
}

fn internal_error(error: &anyhow::Error) -> JsonResponse {
    tracing::error!(
        error = format!("{error:#}"),
        "temporary token signing failed"
    );
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use auth_token::{sign_token, verify_signature};
    use http_server::{bearer_token, testing::StubServer};
    use hyper::header::{ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_ORIGIN};

    use super::*;

    const SECRET: &str = "temporary-secret";

    struct TestServer {
        base_url: String,
        _server: TemporaryAppsServer,
    }

    async fn start_server(max_issues: u32, bots: Option<BotProxy>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        TestServer {
            base_url,
            _server: TemporaryAppsServer::run(
                listener,
                TemporaryApps::new(SECRET.into()),
                RateLimit::new(max_issues, Duration::from_secs(3600)),
                1,
                bots,
            ),
        }
    }

    async fn post(
        server: &TestServer,
        path: &str,
        forwarded_for: &str,
        body: impl Into<reqwest::Body>,
    ) -> reqwest::Response {
        reqwest::Client::new()
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
        let server = start_server(10, None).await;

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
        let server = start_server(1, None).await;
        post(&server, "/tokens", "192.0.2.1", "").await;

        // Act
        let response = post(&server, "/tokens", "192.0.2.1", "").await;

        // Assert
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn post_tokens_counts_the_address_the_trusted_proxy_appended() {
        // Arrange
        let server = start_server(1, None).await;
        post(&server, "/tokens", "198.51.100.7, 192.0.2.1", "").await;

        // Act
        let response = post(&server, "/tokens", "203.0.113.9, 192.0.2.1", "").await;

        // Assert: a client-written first entry does not make it another client
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn post_tokens_refresh_returns_200_with_a_token_for_the_same_app() {
        // Arrange
        let server = start_server(10, None).await;
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
        let server = start_server(10, None).await;
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
        let server = start_server(10, None).await;

        // Act
        let response = post(&server, "/tokens/refresh", "192.0.2.1", "{}").await;

        // Assert
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn options_answers_the_preflight() {
        // Arrange
        let server = start_server(10, None).await;

        // Act
        let response = reqwest::Client::new()
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
            "content-type, authorization"
        );
    }

    /// A `bot-dispatcher` that answers every call `201 {"id": "b1"}`.
    async fn fake_bots() -> StubServer {
        StubServer::run(StatusCode::CREATED, r#"{"id":"b1"}"#).await
    }

    /// Lets temporary apps start `recorder` bots on `bot_dispatcher_url`.
    fn recorder_proxy(bot_dispatcher_url: &str, max_launches: u32) -> Option<BotProxy> {
        Some(BotProxy::new(
            bot_dispatcher_url.to_string(),
            vec!["recorder".to_string()],
            RateLimit::new(max_launches, Duration::from_secs(3600)),
        ))
    }

    fn page_token() -> (String, String) {
        let issued = TemporaryApps::new(SECRET.into()).issue(unix_now()).unwrap();
        (issued.token, issued.app_id)
    }

    #[tokio::test]
    async fn a_page_starts_a_bot_through_a_launch_token_for_its_app() {
        // Arrange
        let bots = fake_bots().await;
        let server = start_server(10, recorder_proxy(bots.url(), 10)).await;
        let (token, app_id) = page_token();

        // Act
        let response = reqwest::Client::new()
            .post(format!("{}/bots/recorder", server.base_url))
            .bearer_auth(&token)
            .json(&json!({ "namespace_prefix": format!("{app_id}/room") }))
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({ "id": "b1" })
        );
        let calls = bots.requests();
        assert_eq!(
            (calls[0].method.as_str(), calls[0].uri.as_str()),
            ("POST", "/bots/recorder")
        );
        let launch_token = bearer_token(&calls[0].headers).unwrap();
        verify_signature(launch_token, SECRET).unwrap();
        let claims = auth_token::decode_unverified_payload(launch_token).unwrap();
        assert_eq!(
            (claims["appId"].as_str(), claims["bots"].as_str()),
            (Some(app_id.as_str()), Some(""))
        );
    }

    #[tokio::test]
    async fn only_a_valid_page_token_reaches_the_app_server_routes_of_an_allowed_type() {
        // Arrange
        let bots = fake_bots().await;
        let server = start_server(10, recorder_proxy(bots.url(), 10)).await;
        let (token, _) = page_token();
        let client = reqwest::Client::new();
        let status = |request: reqwest::RequestBuilder| async move {
            request.send().await.unwrap().status()
        };

        // Act
        let without_token = status(client.get(format!("{}/bots/recorder", server.base_url))).await;
        let forged = status(
            client
                .get(format!("{}/bots/recorder", server.base_url))
                .bearer_auth(
                    sign_token(
                        &json!({ "appId": "temp-x", "exp": unix_now() + 60 }),
                        "other",
                    )
                    .unwrap(),
                ),
        )
        .await;
        let other_type = status(
            client
                .get(format!("{}/bots/chat", server.base_url))
                .bearer_auth(&token),
        )
        .await;
        let bot_route = status(
            client
                .post(format!("{}/bots/recorder/b1/token", server.base_url))
                .bearer_auth(&token),
        )
        .await;
        let stop = status(
            client
                .delete(format!("{}/bots/recorder/b1", server.base_url))
                .bearer_auth(&token),
        )
        .await;

        // Assert
        assert_eq!(without_token, StatusCode::UNAUTHORIZED);
        assert_eq!(forged, StatusCode::UNAUTHORIZED);
        assert_eq!(other_type, StatusCode::NOT_FOUND);
        assert_eq!(bot_route, StatusCode::NOT_FOUND);
        assert_eq!(stop, StatusCode::CREATED);
        let calls = bots.requests();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            (calls[0].method.as_str(), calls[0].uri.as_str()),
            ("DELETE", "/bots/recorder/b1")
        );
    }

    #[tokio::test]
    async fn a_client_starts_bots_within_its_launch_limit() {
        // Arrange
        let bots = fake_bots().await;
        let server = start_server(10, recorder_proxy(bots.url(), 1)).await;
        let (token, app_id) = page_token();
        let start = || {
            reqwest::Client::new()
                .post(format!("{}/bots/recorder", server.base_url))
                .bearer_auth(&token)
                .json(&json!({ "namespace_prefix": format!("{app_id}/room") }))
                .send()
        };

        // Act
        let first = start().await.unwrap().status();
        let second = start().await.unwrap().status();

        // Assert
        assert_eq!(
            (first, second),
            (StatusCode::CREATED, StatusCode::TOO_MANY_REQUESTS)
        );
    }

    #[tokio::test]
    async fn a_page_cannot_keep_a_bot_in_an_empty_room() {
        // Arrange
        let bots = fake_bots().await;
        let server = start_server(10, recorder_proxy(bots.url(), 10)).await;
        let (token, app_id) = page_token();
        let start = |stay_when_empty: bool| {
            reqwest::Client::new()
                .post(format!("{}/bots/recorder", server.base_url))
                .bearer_auth(&token)
                .json(&json!({
                    "namespace_prefix": format!("{app_id}/room"),
                    "stay_when_empty": stay_when_empty,
                }))
                .send()
        };

        // Act
        let staying = start(true).await.unwrap();
        let staying_status = staying.status();
        let staying_body: Value = staying.json().await.unwrap();
        let calls_after_staying = bots.requests().len();
        let leaving = start(false).await.unwrap().status();

        // Assert: refused without calling bot-dispatcher; false is forwarded
        assert_eq!(staying_status, StatusCode::FORBIDDEN);
        assert_eq!(staying_body, json!({ "error": "forbidden" }));
        assert_eq!(calls_after_staying, 0);
        assert_eq!(leaving, StatusCode::CREATED);
    }
}

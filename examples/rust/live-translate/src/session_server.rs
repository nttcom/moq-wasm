use std::{convert::Infallible, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::stream;
use http_body_util::{BodyExt, Full, StreamBody, combinators::BoxBody};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Frame, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HeaderValue,
    },
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpListener, task::JoinHandle, time::interval};
use tracing::Instrument;

use crate::temporary_apps::{Refusal, TemporaryApps};

const EVENTS_HEARTBEAT: Duration = Duration::from_secs(15);

/// What a streamer's page asks of the translator: to translate its temporary
/// app (`/sessions`), and to stay up while it broadcasts. A Cloud Run instance
/// lives while a request is in flight, so the `/events` streams the pages hold
/// are what keeps the translator running.
pub(crate) struct SessionServer {
    accept_handle: JoinHandle<()>,
}

pub(crate) struct SessionServerState {
    pub(crate) temporary_apps: TemporaryApps,
}

impl SessionServer {
    pub(crate) fn run(listener: TcpListener, state: SessionServerState) -> Self {
        let state = Arc::new(state);
        let accept_handle = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        tracing::warn!(%error, "accept failed");
                        continue;
                    }
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let state = state.clone();
                        async move { Ok::<_, Infallible>(handle(request, state).await) }
                    });
                    if let Err(error) = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                    {
                        tracing::debug!(%error, "connection closed with error");
                    }
                });
            }
        });
        Self { accept_handle }
    }
}

impl Drop for SessionServer {
    fn drop(&mut self) {
        self.accept_handle.abort();
    }
}

type ServerResponse = Response<BoxBody<Bytes, Infallible>>;

async fn handle(request: Request<Incoming>, state: Arc<SessionServerState>) -> ServerResponse {
    let (head, _body) = request.into_parts();
    let span = tracing::info_span!(
        "live_translate.request",
        "http.request.method" = %head.method,
        "url.path" = head.uri.path(),
        "http.response.status_code" = tracing::field::Empty,
    );
    let mut response = async {
        match (&head.method, head.uri.path()) {
            (&Method::GET, "/healthz") => json_response(StatusCode::OK, r#"{"status":"ok"}"#),
            (&Method::GET, "/events") => events(),
            (&Method::POST, "/sessions") => start_session(&state, &head.headers).await,
            (&Method::OPTIONS, "/sessions") => empty_response(StatusCode::NO_CONTENT),
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    // The pages are served from another origin.
    let headers = response.headers_mut();
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization"),
    );
    response
}

async fn start_session(state: &SessionServerState, headers: &HeaderMap) -> ServerResponse {
    let Some(token) = bearer_token(headers) else {
        return error_response(StatusCode::UNAUTHORIZED, "missing_token");
    };
    match state.temporary_apps.start(token).await {
        Ok(()) => empty_response(StatusCode::NO_CONTENT),
        Err(Refusal::NotATemporaryAppToken) => {
            error_response(StatusCode::FORBIDDEN, "not_a_temporary_app_token")
        }
        Err(Refusal::Expired) => error_response(StatusCode::UNAUTHORIZED, "expired_token"),
        Err(Refusal::Full) => error_response(StatusCode::SERVICE_UNAVAILABLE, "full"),
        Err(Refusal::Relay(error)) => {
            tracing::warn!(?error, "connect to the relay for a temporary app");
            error_response(StatusCode::BAD_GATEWAY, "relay_refused")
        }
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

fn events() -> ServerResponse {
    let heartbeats = stream::unfold(interval(EVENTS_HEARTBEAT), |mut ticks| async move {
        ticks.tick().await;
        let heartbeat = Frame::data(Bytes::from_static(b": alive\n\n"));
        Some((Ok::<_, Infallible>(heartbeat), ticks))
    });
    let mut response = Response::new(StreamBody::new(heartbeats).boxed());
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn empty_response(status: StatusCode) -> ServerResponse {
    let mut response = Response::new(Full::default().boxed());
    *response.status_mut() = status;
    response
}

fn json_response(status: StatusCode, body: &str) -> ServerResponse {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())).boxed());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn error_response(status: StatusCode, error: &str) -> ServerResponse {
    json_response(status, &serde_json::json!({ "error": error }).to_string())
}

#[cfg(test)]
mod tests {
    use tokio::sync::Semaphore;

    use crate::{
        root_translation::{TranslationPolicy, TranslationResources},
        temporary_apps::{tests::unsigned_token, unix_now},
        translation_engine::TranslationEngine,
    };

    use super::*;

    async fn serve() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let policy = Arc::new(TranslationPolicy {
            language_time_limit: Duration::from_secs(3600),
            always_on: Vec::new(),
        });
        let server = SessionServer::run(
            listener,
            SessionServerState {
                // No place for an app, so no test dials the relay.
                temporary_apps: TemporaryApps::new(
                    "moqt://relay.example:443".to_string(),
                    false,
                    TranslationResources {
                        engine: TranslationEngine::Echo,
                        capacity: Arc::new(Semaphore::new(1)),
                        policy,
                    },
                    0,
                ),
            },
        );
        std::mem::forget(server);
        port
    }

    #[tokio::test]
    async fn the_event_stream_is_held() {
        // Arrange
        let port = serve().await;

        // Act
        let response = reqwest::get(format!("http://127.0.0.1:{port}/events"))
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
    }

    async fn post_session(port: u16, token: Option<&str>) -> reqwest::Response {
        let request = reqwest::Client::new().post(format!("http://127.0.0.1:{port}/sessions"));
        match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
        .send()
        .await
        .unwrap()
    }

    fn token_of(app_id: &str) -> String {
        unsigned_token(app_id, unix_now() + 3600)
    }

    #[tokio::test]
    async fn a_session_request_without_a_token_is_unauthorized_for_any_origin() {
        // Arrange
        let port = serve().await;

        // Act
        let response = post_session(port, None).await;

        // Assert
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    }

    #[tokio::test]
    async fn a_session_request_with_a_registered_app_token_is_forbidden() {
        // Arrange
        let port = serve().await;

        // Act
        let response = post_session(port, Some(&token_of("app-1"))).await;

        // Assert
        assert_eq!(response.status(), 403);
    }

    #[tokio::test]
    async fn a_session_request_beyond_the_app_limit_is_unavailable() {
        // Arrange: the server translates no app at once
        let port = serve().await;

        // Act
        let response = post_session(port, Some(&token_of("temp-3f9a"))).await;

        // Assert
        assert_eq!(response.status(), 503);
    }

    #[tokio::test]
    async fn a_preflight_allows_the_authorization_header() {
        // Arrange
        let port = serve().await;

        // Act
        let response = reqwest::Client::new()
            .request(Method::OPTIONS, format!("http://127.0.0.1:{port}/sessions"))
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), 204);
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_HEADERS],
            "authorization"
        );
    }
}

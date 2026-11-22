use std::{convert::Infallible, sync::Arc, time::Duration};

use bytes::Bytes;
use futures_util::stream;
use http_body_util::{BodyExt, StreamBody, combinators::BoxBody};
use http_server::{HttpServer, allow_any_origin, bearer_token};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Frame, Incoming},
    header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue},
};
use tokio::{net::TcpListener, time::interval};
use tracing::Instrument;

use crate::temporary_apps::{Refusal, TemporaryApps};

const EVENTS_HEARTBEAT: Duration = Duration::from_secs(15);

/// What a streamer's page asks of the translator: to translate its temporary
/// app (`/sessions`), and to stay up while it broadcasts. A Cloud Run instance
/// lives while a request is in flight, so the `/events` streams the pages hold
/// are what keeps the translator running.
pub(crate) struct SessionServer {
    _http: HttpServer,
}

pub(crate) struct SessionServerState {
    pub(crate) temporary_apps: TemporaryApps,
}

impl SessionServer {
    pub(crate) fn run(listener: TcpListener, state: SessionServerState) -> Self {
        let state = Arc::new(state);
        Self {
            _http: HttpServer::run(listener, move |request, _peer| {
                handle(request, state.clone())
            }),
        }
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
    telemetry_otlp::continue_trace_from(&span, &head.headers);
    let response = async {
        match (&head.method, head.uri.path()) {
            (&Method::GET, "/healthz") => http_server::healthz_response().map(BodyExt::boxed),
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
    allow_any_origin(response, "GET, POST, OPTIONS", "authorization")
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

fn empty_response(status: StatusCode) -> ServerResponse {
    http_server::empty_response(status).map(BodyExt::boxed)
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

fn error_response(status: StatusCode, error: &str) -> ServerResponse {
    http_server::error_response(status, error).map(BodyExt::boxed)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use auth_token::{Claims, sign_token};
    use hyper::header::{ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_ORIGIN};
    use tokio::sync::Semaphore;

    use crate::{
        root_translation::TranslationResources, translation_engine::TranslationEngine,
        translation_policy::TranslationPolicy,
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
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = Claims::issued(
            app_id.to_string(),
            Some(String::new()),
            Some(String::new()),
            now,
            Duration::from_secs(3600),
        );
        sign_token(&claims, "s3cret").unwrap()
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

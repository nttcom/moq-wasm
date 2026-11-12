use std::{
    convert::Infallible,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use http_server::HttpServer;
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Body, Frame, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HeaderValue,
    },
};
use serde_json::json;
use tokio::{net::TcpListener, time::Interval};
use tracing::Instrument;

use crate::{
    temporary_apps::{Refusal, TemporaryApps},
    translation_policy::TranslationPolicy,
};

const EVENTS_HEARTBEAT: Duration = Duration::from_secs(15);

/// What a streamer's page asks of the translator: to translate its temporary
/// app (`/sessions`), and to stay up while it broadcasts. A Cloud Run instance
/// lives while a request is in flight, so the `/events` streams the pages hold
/// are what keeps the translator running.
pub(crate) struct SessionServer {
    _http: HttpServer,
}

pub(crate) struct SessionServerState {
    pub(crate) policy: Arc<TranslationPolicy>,
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
    let response = async {
        match (&head.method, head.uri.path()) {
            (&Method::GET, "/healthz") => json_response(StatusCode::OK, json!({ "status": "ok" })),
            (&Method::GET, "/events") => events(&state),
            (&Method::POST, "/sessions") => start_session(&state, &head.headers).await,
            (&Method::OPTIONS, "/sessions") => empty_response(StatusCode::NO_CONTENT),
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    with_cors(response)
}

/// The pages are served from another origin, so every response allows any
/// origin, as the token mint's do.
fn with_cors(mut response: ServerResponse) -> ServerResponse {
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
    let Some(token) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return error_response(StatusCode::UNAUTHORIZED, "missing_token");
    };
    match state.temporary_apps.start(token).await {
        Ok(()) => json_response(
            StatusCode::OK,
            json!({
                "openHours": state.policy.open_hours.as_ref().map(ToString::to_string),
                "open": state.policy.is_open(),
            }),
        ),
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
    let mut response = Response::new(Full::default().boxed());
    *response.status_mut() = status;
    response
}

/// `GET /events`: a server-sent event stream of heartbeat comments only; a
/// page holds it open for the reason on `SessionServer`. Outside the open hours
/// nothing is translated, so the stream ends and a new one gets 204. 204
/// tells an `EventSource` not to reconnect, so a tab left open lets the
/// instance stop.
fn events(state: &SessionServerState) -> ServerResponse {
    if !state.policy.is_open() {
        return empty_response(StatusCode::NO_CONTENT);
    }
    let mut response = Response::new(Heartbeat::new(state.policy.clone()).boxed());
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

struct Heartbeat {
    ticks: Interval,
    policy: Arc<TranslationPolicy>,
}

impl Heartbeat {
    fn new(policy: Arc<TranslationPolicy>) -> Self {
        Self {
            ticks: tokio::time::interval(EVENTS_HEARTBEAT),
            policy,
        }
    }
}

impl Body for Heartbeat {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        ready!(self.ticks.poll_tick(cx));
        if !self.policy.is_open() {
            return Poll::Ready(None);
        }
        Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b": alive\n\n")))))
    }
}

fn json_response(status: StatusCode, body: serde_json::Value) -> ServerResponse {
    http_server::json_response(status, &body).map(BodyExt::boxed)
}

fn error_response(status: StatusCode, error: &str) -> ServerResponse {
    http_server::error_response(status, error).map(BodyExt::boxed)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use auth_token::{Claims, sign_token};
    use tokio::sync::Semaphore;

    use crate::{root_translation::TranslationResources, translation_engine::TranslationEngine};

    use super::*;

    async fn serve(open_hours: Option<&str>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let policy = Arc::new(TranslationPolicy {
            open_hours: open_hours.map(|text| text.parse().unwrap()),
            language_time_limit: Duration::from_secs(3600),
            always_on: Vec::new(),
        });
        let server = SessionServer::run(
            listener,
            SessionServerState {
                policy: policy.clone(),
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

    /// Hours that are open only on the weekday three days from now.
    fn closed_hours() -> String {
        let days = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 86_400;
        // 1970-01-01 was a Thursday, so indexing from Sunday names the weekday three days ahead.
        let weekday = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][(days % 7) as usize];
        format!("{weekday}-{weekday} 00:00-23:59 +00:00")
    }

    #[tokio::test]
    async fn the_event_stream_is_held_inside_the_open_hours() {
        // Arrange: no open hours, so always open
        let port = serve(None).await;

        // Act
        let response = reqwest::get(format!("http://127.0.0.1:{port}/events"))
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
    }

    #[tokio::test]
    async fn the_event_stream_is_refused_outside_the_open_hours() {
        // Arrange
        let port = serve(Some(&closed_hours())).await;

        // Act
        let response = reqwest::get(format!("http://127.0.0.1:{port}/events"))
            .await
            .unwrap();

        // Assert: 204 stops an EventSource from reconnecting
        assert_eq!(response.status(), 204);
    }

    #[tokio::test]
    async fn a_held_event_stream_ends_when_the_open_hours_are_over() {
        // Arrange
        let mut heartbeat = Heartbeat::new(Arc::new(TranslationPolicy {
            open_hours: Some(closed_hours().parse().unwrap()),
            language_time_limit: Duration::from_secs(3600),
            always_on: Vec::new(),
        }));

        // Act
        let frame = heartbeat.frame().await;

        // Assert
        assert!(frame.is_none());
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
        let port = serve(None).await;

        // Act
        let response = post_session(port, None).await;

        // Assert
        assert_eq!(response.status(), 401);
        assert_eq!(response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    }

    #[tokio::test]
    async fn a_session_request_with_a_registered_app_token_is_forbidden() {
        // Arrange
        let port = serve(None).await;

        // Act
        let response = post_session(port, Some(&token_of("app-1"))).await;

        // Assert
        assert_eq!(response.status(), 403);
    }

    #[tokio::test]
    async fn a_session_request_beyond_the_app_limit_is_unavailable() {
        // Arrange: the server translates no app at once
        let port = serve(None).await;

        // Act
        let response = post_session(port, Some(&token_of("temp-3f9a"))).await;

        // Assert
        assert_eq!(response.status(), 503);
    }

    #[tokio::test]
    async fn a_preflight_allows_the_authorization_header() {
        // Arrange
        let port = serve(None).await;

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

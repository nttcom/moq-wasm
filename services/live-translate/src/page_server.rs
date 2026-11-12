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
    Method, Request, Response, StatusCode,
    body::{Body, Frame, Incoming},
    header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue, LOCATION},
};
use serde_json::json;
use static_files::StaticFiles;
use tokio::{net::TcpListener, time::Interval};
use tracing::Instrument;

use crate::{
    app_access::AppAccess, broadcast_namespace::BroadcastNamespaces,
    translation_policy::TranslationPolicy,
};

const STREAMER_PAGE: &str = "/examples/live-translate/streamer/";
const EVENTS_HEARTBEAT: Duration = Duration::from_secs(15);
const MAX_BROADCAST_ID_CHARS: usize = 64;

/// The pages, their relay tokens and the stream a page holds open while it is
/// in use. A Cloud Run instance lives while a request is in flight, so the
/// `/events` streams the pages hold are what keeps the translator running
/// between the pages' other requests.
pub(crate) struct PageServer {
    _http: HttpServer,
}

pub(crate) struct PageServerState {
    pub(crate) static_files: Option<StaticFiles>,
    pub(crate) app: Option<Arc<AppAccess>>,
    pub(crate) namespaces: BroadcastNamespaces,
    pub(crate) policy: Arc<TranslationPolicy>,
    /// The relay URL the pages connect to, which may differ from the
    /// translator's own.
    pub(crate) public_relay_url: String,
}

impl PageServer {
    pub(crate) fn run(listener: TcpListener, state: PageServerState) -> Self {
        let state = Arc::new(state);
        Self {
            _http: HttpServer::run(listener, move |request, _peer| {
                handle(request, state.clone())
            }),
        }
    }
}

type PageResponse = Response<BoxBody<Bytes, Infallible>>;

async fn handle(request: Request<Incoming>, state: Arc<PageServerState>) -> PageResponse {
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
            (&Method::GET, "/token") => issue_token(&state, head.uri.query()),
            (&Method::GET, "/events") => events(&state),
            (&Method::GET, "/") => redirect(STREAMER_PAGE),
            (&Method::GET, path) => static_file(&state, path).await,
            _ => error_response(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    response
}

/// `GET /token?id=<broadcast id>&role=streamer|viewer`: a token to publish
/// (streamer) or subscribe (viewer) the broadcast, with the namespace it lives
/// in and the relay to connect to. It also carries the open hours (`null`
/// when unlimited) and whether they are open now, so a page can say so before
/// a viewer picks a language. Refused without an app, where the broadcasts
/// are public and need no token.
fn issue_token(state: &PageServerState, query: Option<&str>) -> PageResponse {
    let Some(app) = &state.app else {
        return error_response(StatusCode::NOT_FOUND, "no_app");
    };
    let Some((broadcast_id, role)) = token_request(query) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let (publish, subscribe) = match role {
        Role::Streamer => (Some(broadcast_id), None),
        Role::Viewer => (None, Some(broadcast_id)),
    };
    let token = match app.issue(publish, subscribe) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(?error, "sign relay token");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        }
    };
    tracing::info!(broadcast_id, role = role.as_str(), "relay token issued");
    json_response(
        StatusCode::OK,
        json!({
            "token": token,
            "namespace": format!("{}/{broadcast_id}", state.namespaces.root()),
            "relayUrl": state.public_relay_url,
            "openHours": state.policy.open_hours.as_ref().map(ToString::to_string),
            "open": state.policy.is_open(),
        }),
    )
}

#[derive(Clone, Copy)]
enum Role {
    Streamer,
    Viewer,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Streamer => "streamer",
            Role::Viewer => "viewer",
        }
    }
}

fn token_request(query: Option<&str>) -> Option<(&str, Role)> {
    let mut broadcast_id = None;
    let mut role = None;
    for pair in query?.split('&') {
        match pair.split_once('=')? {
            ("id", value) if is_broadcast_id(value) => broadcast_id = Some(value),
            ("role", "streamer") => role = Some(Role::Streamer),
            ("role", "viewer") => role = Some(Role::Viewer),
            _ => return None,
        }
    }
    Some((broadcast_id?, role?))
}

/// One namespace element: the ids the streamer page generates and nothing a
/// path or a query would need escaping for.
fn is_broadcast_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_BROADCAST_ID_CHARS
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

/// `GET /events`: a server-sent event stream of heartbeat comments only; a
/// page holds it open for the reason on `PageServer`. Outside the open hours
/// nothing is translated, so the stream ends and a new one gets 204. 204
/// tells an `EventSource` not to reconnect, so a tab left open lets the
/// instance stop.
fn events(state: &PageServerState) -> PageResponse {
    if !state.policy.is_open() {
        let mut response = Response::new(Full::default().boxed());
        *response.status_mut() = StatusCode::NO_CONTENT;
        return response;
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

async fn static_file(state: &PageServerState, path: &str) -> PageResponse {
    let Some(static_files) = &state.static_files else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let file_path = if path.ends_with('/') {
        format!("{path}index.html")
    } else {
        path.to_string()
    };
    let Some(file) = static_files.get(&file_path).await else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let cache_control = file.cache_control();
    let mut response = Response::new(Full::from(file.bytes).boxed());
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(file.content_type));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    response
}

fn redirect(location: &'static str) -> PageResponse {
    let mut response = Response::new(Full::from(Bytes::new()).boxed());
    *response.status_mut() = StatusCode::FOUND;
    response
        .headers_mut()
        .insert(LOCATION, HeaderValue::from_static(location));
    response
}

fn json_response(status: StatusCode, body: serde_json::Value) -> PageResponse {
    http_server::json_response(status, &body).map(BodyExt::boxed)
}

fn error_response(status: StatusCode, error: &str) -> PageResponse {
    http_server::error_response(status, error).map(BodyExt::boxed)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use auth_token::{decode_unverified_payload, verify_signature};

    use super::*;

    async fn serve(app: Option<Arc<AppAccess>>, open_hours: Option<&str>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = PageServer::run(
            listener,
            PageServerState {
                static_files: None,
                app,
                namespaces: BroadcastNamespaces::new("app-1".to_string()),
                policy: Arc::new(TranslationPolicy {
                    open_hours: open_hours.map(|text| text.parse().unwrap()),
                    language_time_limit: Duration::from_secs(3600),
                    always_on: Vec::new(),
                }),
                public_relay_url: "https://relay.example:443".to_string(),
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

    fn app() -> Arc<AppAccess> {
        Arc::new(AppAccess::new("app-1".to_string(), "s3cret".to_string()))
    }

    #[tokio::test]
    async fn a_viewer_token_subscribes_its_broadcast_under_the_app() {
        // Arrange
        let port = serve(Some(app()), None).await;

        // Act
        let body: serde_json::Value = reqwest::get(format!(
            "http://127.0.0.1:{port}/token?id=brave-otter&role=viewer"
        ))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

        // Assert
        let token = body["token"].as_str().unwrap();
        verify_signature(token, "s3cret").unwrap();
        let payload = decode_unverified_payload(token).unwrap();
        assert_eq!(payload["appId"], "app-1");
        assert_eq!(payload["subscribe"], "brave-otter");
        assert!(payload.get("publish").is_none());
        assert_eq!(body["namespace"], "app-1/brave-otter");
        assert_eq!(body["relayUrl"], "https://relay.example:443");
        assert!(body["openHours"].is_null());
        assert_eq!(body["open"], true);
    }

    #[tokio::test]
    async fn a_token_carries_the_open_hours() {
        // Arrange
        let port = serve(Some(app()), Some("Mon-Fri 09:00-17:30 +09:00")).await;

        // Act
        let body: serde_json::Value = reqwest::get(format!(
            "http://127.0.0.1:{port}/token?id=brave-otter&role=viewer"
        ))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

        // Assert: the text as given; `open` depends on the clock, so only its type
        assert_eq!(body["openHours"], "Mon-Fri 09:00-17:30 +09:00");
        assert!(body["open"].is_boolean());
    }

    #[tokio::test]
    async fn without_an_app_no_token_is_issued() {
        // Arrange
        let port = serve(None, None).await;

        // Act
        let response = reqwest::get(format!(
            "http://127.0.0.1:{port}/token?id=brave-otter&role=viewer"
        ))
        .await
        .unwrap();

        // Assert
        assert_eq!(response.status(), 404);
    }

    #[tokio::test]
    async fn the_event_stream_is_held_inside_the_open_hours() {
        // Arrange: no open hours, so always open
        let port = serve(Some(app()), None).await;

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
        let port = serve(Some(app()), Some(&closed_hours())).await;

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

    #[test]
    fn a_token_request_names_a_broadcast_and_a_role() {
        // Act / Assert
        assert!(matches!(
            token_request(Some("id=brave-otter&role=viewer")),
            Some(("brave-otter", Role::Viewer))
        ));
        assert!(matches!(
            token_request(Some("role=streamer&id=brave-otter")),
            Some(("brave-otter", Role::Streamer))
        ));
    }

    #[test]
    fn a_token_request_with_an_unknown_part_or_an_odd_id_is_refused() {
        // Act / Assert
        for query in [
            None,
            Some("id=brave-otter"),
            Some("role=viewer"),
            Some("id=brave-otter&role=admin"),
            Some("id=brave%2Fotter&role=viewer"),
            Some("id=a/b&role=viewer"),
            Some("id=brave-otter&role=viewer&extra=1"),
        ] {
            assert!(token_request(query).is_none(), "{query:?}");
        }
    }
}

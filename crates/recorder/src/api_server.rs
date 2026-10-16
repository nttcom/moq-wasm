use std::{collections::HashMap, convert::Infallible, sync::Arc};

use anyhow::{Context, Result};
use clap::ValueEnum;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        CONTENT_TYPE, HeaderValue, ORIGIN, VARY,
    },
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    recording::Layout,
    recordings::{Recordings, StartRejection},
};

const MAX_BODY_BYTES: usize = 16 * 1024;

/// Serves the recording API:
/// - `POST /recordings` with `{"namespace_prefix", "layout"}` starts recording
///   every broadcast under the prefix; `layout` is `combined` by default
/// - `POST /recordings/{id}/stop` stops it
/// - `GET /recordings/{id}` and `GET /recordings?namespace_prefix=` report it
///
/// A POST from a page outside `allowed_origins` is refused: CORS alone would
/// only hide the answer from that page, and a form or a `text/plain` fetch
/// needs no preflight. Nothing authenticates a caller, so `Recordings` keeps
/// every request under one namespace root.
pub(crate) struct ApiServer {
    handle: JoinHandle<()>,
}

struct ApiState {
    recordings: Arc<Recordings>,
    allowed_origins: Vec<String>,
}

impl ApiServer {
    pub(crate) fn run(
        listener: TcpListener,
        recordings: Arc<Recordings>,
        allowed_origins: Vec<String>,
    ) -> Self {
        let state = Arc::new(ApiState {
            recordings,
            allowed_origins,
        });
        Self {
            handle: tokio::spawn(accept_connections(listener, state)),
        }
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn accept_connections(listener: TcpListener, state: Arc<ApiState>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                continue;
            }
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, Arc::clone(&state)));
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

async fn handle(
    request: Request<Incoming>,
    state: Arc<ApiState>,
) -> Result<JsonResponse, Infallible> {
    let origin = allowed_origin(request.headers(), &state.allowed_origins);
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = query_params(&request);
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let foreign_page = request.headers().contains_key(ORIGIN) && origin.is_none();
    let response = match (&method, segments.as_slice()) {
        (&Method::POST, _) if foreign_page => {
            error_response(StatusCode::FORBIDDEN, "origin not allowed")
        }
        (&Method::OPTIONS, _) => empty_response(StatusCode::NO_CONTENT),
        (&Method::GET, ["healthz"]) => json_response(StatusCode::OK, &json!({ "status": "ok" })),
        (&Method::POST, ["recordings"]) => match read_json(request).await {
            Ok(body) => start(&state.recordings, &body),
            Err(error) => error_response(StatusCode::BAD_REQUEST, &format!("{error:#}")),
        },
        (&Method::GET, ["recordings"]) => json_response(
            StatusCode::OK,
            &state
                .recordings
                .list(query.get("namespace_prefix").map(String::as_str)),
        ),
        (&Method::GET, ["recordings", id]) => found(state.recordings.get(id)),
        (&Method::POST, ["recordings", id, "stop"]) => match state.recordings.stop(id) {
            Some(recording) => json_response(StatusCode::ACCEPTED, &recording),
            None => error_response(StatusCode::NOT_FOUND, "no such recording"),
        },
        _ => error_response(StatusCode::NOT_FOUND, "not_found"),
    };
    Ok(with_cors(response, origin))
}

fn start(recordings: &Recordings, body: &Value) -> JsonResponse {
    let Some(namespace_prefix) = body.get("namespace_prefix").and_then(Value::as_str) else {
        return error_response(StatusCode::BAD_REQUEST, "namespace_prefix is required");
    };
    let layout = match body.get("layout").and_then(Value::as_str) {
        None => Layout::Combined,
        Some(name) => match Layout::from_str(name, false) {
            Ok(layout) => layout,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "layout must be combined or per-track",
                );
            }
        },
    };
    match recordings.start(namespace_prefix, layout) {
        Ok(recording) => json_response(StatusCode::CREATED, &recording),
        Err(StartRejection::OutsideRoot) => error_response(
            StatusCode::FORBIDDEN,
            "namespace_prefix must name a room under the allowed root",
        ),
        Err(StartRejection::AlreadyRecording) => {
            error_response(StatusCode::CONFLICT, "the room is already being recorded")
        }
        Err(StartRejection::TooManyRecordings) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "too many recordings at once",
        ),
    }
}

async fn read_json(request: Request<Incoming>) -> Result<Value> {
    let body = Limited::new(request.into_body(), MAX_BODY_BYTES)
        .collect()
        .await
        .map_err(|error| anyhow::anyhow!("cannot read the body: {error}"))?
        .to_bytes();
    serde_json::from_slice(&body).context("the body is not JSON")
}

fn query_params(request: &Request<Incoming>) -> HashMap<String, String> {
    request
        .uri()
        .query()
        .and_then(|query| url::Url::parse(&format!("http://query/?{query}")).ok())
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

fn allowed_origin(headers: &HeaderMap, allowed: &[String]) -> Option<HeaderValue> {
    let origin = headers.get(ORIGIN)?;
    let origin_str = origin.to_str().ok()?;
    allowed
        .iter()
        .any(|allowed| allowed == origin_str)
        .then(|| origin.clone())
}

fn with_cors(mut response: JsonResponse, origin: Option<HeaderValue>) -> JsonResponse {
    let headers = response.headers_mut();
    headers.insert(VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = origin {
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        headers.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Content-Type"),
        );
    }
    response
}

fn found(value: Option<Value>) -> JsonResponse {
    match value {
        Some(value) => json_response(StatusCode::OK, &value),
        None => error_response(StatusCode::NOT_FOUND, "no such recording"),
    }
}

fn json_response(status: StatusCode, body: &Value) -> JsonResponse {
    let mut response = Response::new(Full::new(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn empty_response(status: StatusCode) -> JsonResponse {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = status;
    response
}

fn error_response(status: StatusCode, message: &str) -> JsonResponse {
    json_response(status, &json!({ "error": message }))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{recordings::ServeConfig, room_recording::RecordingLimits};

    fn recordings() -> Recordings {
        Recordings::new(ServeConfig {
            relay: "moqt://127.0.0.1:1".to_string(),
            verify_certificate: true,
            namespace_root: "anon".to_string(),
            max_recordings: 1,
            output_dir: std::env::temp_dir(),
            upload: None,
            max_video_height: 480,
            segment_length: Some(Duration::from_secs(60)),
            limits: RecordingLimits {
                max_duration: Duration::from_secs(7200),
                empty_room_timeout: Duration::from_secs(300),
            },
        })
    }

    async fn error_of(response: JsonResponse) -> (StatusCode, Value) {
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn rejects_a_request_without_a_room_under_the_root_or_with_an_unknown_layout() {
        // Arrange
        let recordings = recordings();

        // Act
        let missing = error_of(start(&recordings, &json!({}))).await;
        let outside = error_of(start(
            &recordings,
            &json!({ "namespace_prefix": "app/room" }),
        ))
        .await;
        let layout = error_of(start(
            &recordings,
            &json!({ "namespace_prefix": "anon/room", "layout": "grid" }),
        ))
        .await;

        // Assert: nothing was started
        assert_eq!(missing.0, StatusCode::BAD_REQUEST);
        assert_eq!(outside.0, StatusCode::FORBIDDEN);
        assert_eq!(layout.0, StatusCode::BAD_REQUEST);
        assert_eq!(recordings.list(None), json!({ "recordings": [] }));
    }

    #[test]
    fn answers_cors_only_for_an_allowed_origin() {
        // Arrange
        let allowed = vec!["http://localhost:5173".to_string()];
        let mut from_meeting = HeaderMap::new();
        from_meeting.insert(ORIGIN, HeaderValue::from_static("http://localhost:5173"));
        let mut from_elsewhere = HeaderMap::new();
        from_elsewhere.insert(ORIGIN, HeaderValue::from_static("http://evil.example"));

        // Act / Assert
        assert_eq!(
            allowed_origin(&from_meeting, &allowed),
            Some(HeaderValue::from_static("http://localhost:5173"))
        );
        assert_eq!(allowed_origin(&from_elsewhere, &allowed), None);
        assert_eq!(allowed_origin(&HeaderMap::new(), &allowed), None);
    }
}

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use clap::ValueEnum;
use http_body_util::{BodyExt, Either, Full, Limited};
use http_server::HttpServer;
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        CONTENT_TYPE, HeaderValue, ORIGIN, VARY,
    },
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::{
    api_auth::bearer_token,
    recording::Layout,
    recordings::{Publication, PublishRejection, Recordings, StartRejection},
    replay_stream::ReplayStream,
    room_state::RecordingFormat,
};

const MAX_BODY_BYTES: usize = 16 * 1024;

/// Serves the recording API:
/// - `POST /recordings` with `{"namespace_prefix", "layout", "composite"}`
///   starts recording every broadcast under the prefix; `layout` is
///   `combined` by default. `composite: true` also composites the room into
///   one MP4 once the recording has ended; only a server started with
///   `--composite` accepts it
/// - `POST /recordings/{id}/stop` stops it
/// - `POST /recordings/{id}/publish` exposes a finished recording through relay FETCH
///   while its answer stays open (see [`ReplayStream`])
/// - `GET /recordings/{id}` and `GET /recordings?namespace_prefix=` report it
///
/// A POST from a page outside `allowed_origins` is refused: CORS alone would
/// only hide the answer from that page, and a form or a `text/plain` fetch
/// needs no preflight. Private rooms require a VTS-verified bearer token;
/// anonymous rooms are public. A failing store answers 500.
pub(crate) struct ApiServer {
    _http: HttpServer,
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
            _http: HttpServer::run(listener, move |request, _peer| {
                handle(request, Arc::clone(&state))
            }),
        }
    }
}

type ApiResponse = Response<Either<Full<Bytes>, ReplayStream>>;

async fn handle(request: Request<Incoming>, state: Arc<ApiState>) -> ApiResponse {
    let origin = allowed_origin(request.headers(), &state.allowed_origins);
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = query_params(&request);
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    let foreign_page = request.headers().contains_key(ORIGIN) && origin.is_none();
    let headers = request.headers().clone();
    if method != Method::OPTIONS && path != "/healthz" {
        let namespace = match segments.as_slice() {
            ["recordings", id, ..] => match state.recordings.namespace(id).await {
                Ok(Some(namespace)) => namespace,
                Ok(None) => {
                    return with_cors(
                        error_response(StatusCode::NOT_FOUND, "no such recording"),
                        origin,
                    );
                }
                Err(error) => return with_cors(internal_error(&error), origin),
            },
            ["recordings"] if method == Method::GET => query
                .get("namespace_prefix")
                .cloned()
                .unwrap_or_else(|| state.recordings.namespace_root().to_string()),
            _ => String::new(),
        };
        if !namespace.is_empty() {
            let write = method == Method::POST && segments.last() != Some(&"publish");
            if let Err(error) = state
                .recordings
                .authorize(&headers, &namespace, write)
                .await
            {
                return with_cors(error_response(error.status, error.message), origin);
            }
        }
    }
    let response = match (&method, segments.as_slice()) {
        (&Method::POST, _) if foreign_page => {
            error_response(StatusCode::FORBIDDEN, "origin not allowed")
        }
        (&Method::OPTIONS, _) => empty_response(StatusCode::NO_CONTENT),
        (&Method::GET, ["healthz"]) => json_response(StatusCode::OK, &json!({ "status": "ok" })),
        (&Method::POST, ["recordings"]) => match read_json(request).await {
            Ok(body) => {
                if let Some(namespace) = body["namespace_prefix"].as_str()
                    && let Err(error) = state.recordings.authorize(&headers, namespace, true).await
                {
                    return with_cors(error_response(error.status, error.message), origin);
                }
                start(&state.recordings, &body, bearer_token(&headers)).await
            }
            Err(error) => error_response(StatusCode::BAD_REQUEST, &format!("{error:#}")),
        },
        (&Method::GET, ["recordings"]) => match state
            .recordings
            .list(query.get("namespace_prefix").map(String::as_str))
            .await
        {
            Ok(recordings) => json_response(StatusCode::OK, &recordings),
            Err(error) => internal_error(&error),
        },
        (&Method::GET, ["recordings", id]) => match state.recordings.get(id).await {
            Ok(recording) => found(recording),
            Err(error) => internal_error(&error),
        },
        (&Method::POST, ["recordings", id, "stop"]) => match state.recordings.stop(id).await {
            Ok(Some(recording)) => json_response(StatusCode::ACCEPTED, &recording),
            Ok(None) => error_response(StatusCode::NOT_FOUND, "no such recording"),
            Err(error) => internal_error(&error),
        },
        (&Method::POST, ["recordings", id, "publish"]) => {
            match state.recordings.publish(id, bearer_token(&headers)).await {
                Ok(Some(publication)) => replay_response(publication),
                Ok(None) => error_response(StatusCode::NOT_FOUND, "no such recording"),
                Err(PublishRejection::NotReady) => error_response(
                    StatusCode::CONFLICT,
                    "only finished recordings can be published",
                ),
                Err(PublishRejection::NotMoq) => error_response(
                    StatusCode::CONFLICT,
                    "only recordings in the moq format can be published",
                ),
                Err(PublishRejection::OutsideRoot) => error_response(
                    StatusCode::FORBIDDEN,
                    "the recording's room cannot be replayed here",
                ),
                Err(PublishRejection::Capacity) => error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "too many recording replays",
                ),
                Err(PublishRejection::Internal(error)) => internal_error(&error),
            }
        }
        _ => error_response(StatusCode::NOT_FOUND, "not_found"),
    };
    with_cors(response, origin)
}

async fn start(recordings: &Recordings, body: &Value, bearer: Option<&str>) -> ApiResponse {
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
    let composite = match body.get("composite") {
        None => false,
        Some(Value::Bool(composite)) => *composite,
        Some(_) => return error_response(StatusCode::BAD_REQUEST, "composite must be a boolean"),
    };
    let format = match body.get("format") {
        None => RecordingFormat::Mp4,
        Some(Value::String(name)) => match RecordingFormat::from_str(name, false) {
            Ok(format) => format,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "format must be mp4 or moq"),
        },
        Some(_) => return error_response(StatusCode::BAD_REQUEST, "format must be mp4 or moq"),
    };
    match recordings
        .start(namespace_prefix, layout, composite, format, bearer)
        .await
    {
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
        Err(StartRejection::CompositeDisabled) => error_response(
            StatusCode::BAD_REQUEST,
            "this recorder does not composite; start it with --composite",
        ),
        Err(StartRejection::CompositeRequiresMp4) => {
            error_response(StatusCode::BAD_REQUEST, "composite requires mp4 format")
        }
        Err(StartRejection::Internal(error)) => internal_error(&error),
    }
}

fn internal_error(error: &anyhow::Error) -> ApiResponse {
    tracing::error!(error = format!("{error:#}"), "recording API request failed");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("{error:#}"))
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

fn with_cors(mut response: ApiResponse, origin: Option<HeaderValue>) -> ApiResponse {
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
            HeaderValue::from_static("Content-Type, Authorization"),
        );
    }
    response
}

fn found(value: Option<Value>) -> ApiResponse {
    match value {
        Some(value) => json_response(StatusCode::OK, &value),
        None => error_response(StatusCode::NOT_FOUND, "no such recording"),
    }
}

fn replay_response(publication: Publication) -> ApiResponse {
    let mut response = Response::new(Either::Right(ReplayStream::new(
        &publication.description,
        publication.lease,
    )));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    response
}

fn json_response(status: StatusCode, body: &Value) -> ApiResponse {
    http_server::json_response(status, body).map(Either::Left)
}

fn empty_response(status: StatusCode) -> ApiResponse {
    let mut response = Response::new(Either::Left(Full::new(Bytes::new())));
    *response.status_mut() = status;
    response
}

fn error_response(status: StatusCode, message: &str) -> ApiResponse {
    http_server::error_response(status, message).map(Either::Left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::recordings;

    async fn error_of(response: ApiResponse) -> (StatusCode, Value) {
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn rejects_a_request_without_a_room_under_the_root_or_with_an_unknown_option() {
        // Arrange
        let recordings = recordings(1, false);

        // Act
        let missing = error_of(start(&recordings, &json!({}), None).await).await;
        let outside = error_of(
            start(
                &recordings,
                &json!({ "namespace_prefix": "app/room" }),
                None,
            )
            .await,
        )
        .await;
        let layout = error_of(
            start(
                &recordings,
                &json!({ "namespace_prefix": "anon/room", "layout": "grid" }),
                None,
            )
            .await,
        )
        .await;
        let composite = error_of(
            start(
                &recordings,
                &json!({ "namespace_prefix": "anon/room", "composite": "yes" }),
                None,
            )
            .await,
        )
        .await;
        let composite_disabled = error_of(
            start(
                &recordings,
                &json!({ "namespace_prefix": "anon/room", "composite": true }),
                None,
            )
            .await,
        )
        .await;

        // Assert: nothing was started
        assert_eq!(missing.0, StatusCode::BAD_REQUEST);
        assert_eq!(outside.0, StatusCode::FORBIDDEN);
        assert_eq!(layout.0, StatusCode::BAD_REQUEST);
        assert_eq!(composite.0, StatusCode::BAD_REQUEST);
        assert_eq!(composite_disabled.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            recordings.list(None).await.unwrap(),
            json!({ "recordings": [] })
        );
    }

    #[tokio::test]
    async fn selects_moq_recording_and_rejects_incompatible_composite() {
        // Arrange
        let recordings = recordings(2, true);

        // Act
        let invalid = error_of(
            start(
                &recordings,
                &json!({
                    "namespace_prefix": "anon/room", "format": "moq", "composite": true
                }),
                None,
            )
            .await,
        )
        .await;
        let accepted = error_of(
            start(
                &recordings,
                &json!({
                    "namespace_prefix": "anon/room", "format": "moq"
                }),
                None,
            )
            .await,
        )
        .await;

        // Assert
        assert_eq!(invalid.0, StatusCode::BAD_REQUEST);
        assert_eq!(accepted.0, StatusCode::CREATED);
        assert_eq!(accepted.1["format"], "moq");
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

use std::{collections::HashMap, sync::Arc};

use http_body_util::{Either, Full};
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
    recordings::{Publication, PublishRejection, Recordings},
    replay_stream::ReplayStream,
};

/// Serves the recordings that `recorder bot` jobs made; `bots` starts and
/// stops them (docs/architecture/bots/architecture.md):
/// - `POST /recordings/{id}/publish` exposes a finished recording through relay FETCH
///   while its answer stays open (see [`ReplayStream`])
/// - `GET /recordings/{id}` and `GET /recordings?namespace_prefix=` report it
/// - `GET /apps/{app_id}/recordings` lists the app's MP4 recordings with their
///   files, and `GET /recordings/{id}/file?name=` answers a signed URL that
///   reads one of them from the store
///
/// A POST from a page outside `allowed_origins` is refused: CORS alone would
/// only hide the answer from that page, and a form or a `text/plain` fetch
/// needs no preflight. Every request but `/healthz` and a preflight needs a
/// VTS-verified bearer token whose `subscribe` claim covers the room; an
/// app's list needs a `subscribe` claim covering the whole app. A failing store
/// answers 500.
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
    if method != Method::OPTIONS {
        let namespace = match authorized_namespace(&method, &segments, &query, &state).await {
            Ok(namespace) => namespace,
            Err(Refusal::Status(status, message)) => {
                return with_cors(error_response(status, message), origin);
            }
            Err(Refusal::Internal(error)) => return with_cors(internal_error(&error), origin),
        };
        if let Some(namespace) = namespace
            && let Err(error) = state.recordings.authorize(&headers, &namespace).await
        {
            return with_cors(error_response(error.status, error.message), origin);
        }
    }
    let response = match (&method, segments.as_slice()) {
        (&Method::POST, _) if foreign_page => {
            error_response(StatusCode::FORBIDDEN, "origin not allowed")
        }
        (&Method::OPTIONS, _) => empty_response(StatusCode::NO_CONTENT),
        (&Method::GET, ["healthz"]) => json_response(StatusCode::OK, &json!({ "status": "ok" })),
        (&Method::GET, ["recordings"]) => match state
            .recordings
            .list(query.get("namespace_prefix").map_or("", String::as_str))
            .await
        {
            Ok(recordings) => json_response(StatusCode::OK, &recordings),
            Err(error) => internal_error(&error),
        },
        (&Method::GET, ["apps", app_id, "recordings"]) => {
            match state.recordings.app_recordings(app_id).await {
                Ok(recordings) => json_response(StatusCode::OK, &recordings),
                Err(error) => internal_error(&error),
            }
        }
        (&Method::GET, ["recordings", id, "file"]) => match query.get("name") {
            None => error_response(StatusCode::BAD_REQUEST, "name is required"),
            Some(name) => match state.recordings.file_url(id, name).await {
                Ok(Some(url)) => json_response(StatusCode::OK, &url),
                Ok(None) => error_response(StatusCode::NOT_FOUND, "no such file"),
                Err(error) => internal_error(&error),
            },
        },
        (&Method::GET, ["recordings", id]) => match state.recordings.get(id).await {
            Ok(recording) => found(recording),
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

/// The namespace the request's token must cover. `None` skips the check, so
/// only a route naming no namespace answers it. An empty app or prefix is
/// refused instead.
async fn authorized_namespace(
    method: &Method,
    segments: &[&str],
    query: &HashMap<String, String>,
    state: &ApiState,
) -> Result<Option<String>, Refusal> {
    match segments {
        ["recordings", id, ..] => match state.recordings.namespace(id).await {
            Ok(Some(namespace)) => Ok(Some(namespace)),
            Ok(None) => Err(Refusal::Status(StatusCode::NOT_FOUND, "no such recording")),
            Err(error) => Err(Refusal::Internal(error)),
        },
        ["apps", app_id, "recordings"] => non_empty(app_id, "app_id is required"),
        ["recordings"] if method == Method::GET => non_empty(
            query.get("namespace_prefix").map_or("", String::as_str),
            "namespace_prefix is required",
        ),
        _ => Ok(None),
    }
}

/// Small instead of an `ApiResponse`, which clippy finds too large for an `Err`.
enum Refusal {
    Status(StatusCode, &'static str),
    Internal(anyhow::Error),
}

fn non_empty(namespace: &str, message: &'static str) -> Result<Option<String>, Refusal> {
    if namespace.is_empty() {
        Err(Refusal::Status(StatusCode::BAD_REQUEST, message))
    } else {
        Ok(Some(namespace.to_string()))
    }
}

fn internal_error(error: &anyhow::Error) -> ApiResponse {
    tracing::error!(error = format!("{error:#}"), "recording API request failed");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, &format!("{error:#}"))
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

    #[tokio::test]
    async fn a_route_naming_an_empty_app_or_room_is_refused_before_authorization() {
        // Arrange
        let state = ApiState {
            recordings: Arc::new(recordings(1)),
            allowed_origins: Vec::new(),
        };
        let no_query = HashMap::new();
        let empty_prefix = HashMap::from([("namespace_prefix".to_string(), String::new())]);
        let scope = |segments: &'static [&'static str], query| {
            let state = &state;
            async move {
                match authorized_namespace(&Method::GET, segments, query, state).await {
                    Ok(namespace) => Ok(namespace),
                    Err(Refusal::Status(status, _)) => Err(status),
                    Err(Refusal::Internal(error)) => panic!("{error:#}"),
                }
            }
        };

        // Act / Assert
        assert_eq!(
            scope(&["apps", "", "recordings"], &no_query).await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            scope(&["recordings"], &empty_prefix).await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            scope(&["recordings"], &no_query).await,
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            scope(&["apps", "app", "recordings"], &no_query).await,
            Ok(Some("app".to_string()))
        );
        assert_eq!(scope(&["healthz"], &no_query).await, Ok(None));
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

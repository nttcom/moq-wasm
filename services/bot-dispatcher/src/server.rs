use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use http_server::{
    HttpServer, JsonResponse, bearer_token, empty_response, error_response, healthz_response,
    json_response, query_params, read_json,
};
use hyper::{HeaderMap, Method, Request, StatusCode, body::Incoming};
use serde_json::json;
use tokio::net::TcpListener;
use tracing::Instrument;

use crate::{
    bots::{BotError, Bots, Heartbeat, StartError, StartRequest},
    launch_auth::{LaunchAuth, Rejection},
    store::BotRecord,
};

pub(crate) struct BotDispatcherServer {
    _http: HttpServer,
}

struct ServerState {
    bots: Bots,
    launch_auth: LaunchAuth,
}

impl BotDispatcherServer {
    pub(crate) fn run(listener: TcpListener, bots: Bots, launch_auth: LaunchAuth) -> Self {
        let state = Arc::new(ServerState { bots, launch_auth });
        Self {
            _http: HttpServer::run(listener, move |request, _| {
                handle(request, Arc::clone(&state))
            }),
        }
    }
}

async fn handle(request: Request<Incoming>, state: Arc<ServerState>) -> JsonResponse {
    let (head, body) = request.into_parts();
    let span = tracing::info_span!(
        "bot_dispatcher.request",
        "http.request.method" = %head.method,
        "url.path" = head.uri.path(),
        "http.response.status_code" = tracing::field::Empty,
    );
    telemetry_otlp::continue_trace_from(&span, &head.headers);
    let path = head.uri.path().to_string();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let response = async {
        match (&head.method, segments.as_slice()) {
            (&Method::GET, ["healthz"]) => healthz_response(),
            (&Method::POST, ["bots", bot_type]) => {
                start(&state, &head.headers, bot_type, body).await
            }
            (&Method::GET, ["bots"]) => room(&state, &head.headers, head.uri.query()).await,
            (&Method::GET, ["bots", bot_type]) => {
                latest(&state, &head.headers, bot_type, head.uri.query()).await
            }
            (&Method::GET, ["bots", bot_type, id]) => {
                show(&state, &head.headers, bot_type, id).await
            }
            (&Method::DELETE, ["bots", bot_type, id]) => {
                stop(&state, &head.headers, bot_type, id).await
            }
            (&Method::POST, ["bots", bot_type, id, "token"]) => {
                token(&state, &head.headers, bot_type, id).await
            }
            (&Method::POST, ["bots", bot_type, id, "heartbeat"]) => {
                heartbeat(&state, &head.headers, bot_type, id, body).await
            }
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    response
}

async fn start(
    state: &ServerState,
    headers: &HeaderMap,
    bot_type: &str,
    body: Incoming,
) -> JsonResponse {
    let Some(request) = read_json::<StartRequest, _>(body).await else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let prefix = request.namespace_prefix.as_str();
    let app_id = match state.launch_auth.authorize(headers, prefix).await {
        Ok(app_id) => app_id,
        Err(rejection) => return refused(rejection, prefix),
    };
    let result = state
        .bots
        .start(&app_id, bot_type, &request, now_ms())
        .await;
    let (status, error) = match result {
        Ok(bot) => {
            tracing::info!(
                bot_id = %bot.id,
                bot_type = %bot.bot_type,
                app_id = %app_id,
                track_namespace_prefix = prefix,
                "bot launched"
            );
            return json_response(StatusCode::CREATED, &bot);
        }
        Err(StartError::UnknownType) => (StatusCode::NOT_FOUND, "not_found"),
        Err(StartError::InvalidRoom) => (StatusCode::BAD_REQUEST, "invalid_room"),
        Err(StartError::InvalidOptions) => (StatusCode::BAD_REQUEST, "invalid_options"),
        Err(StartError::Conflict) => (StatusCode::CONFLICT, "conflict"),
        Err(StartError::Full) => (StatusCode::SERVICE_UNAVAILABLE, "full"),
        Err(StartError::Internal(error)) => {
            tracing::error!(
                error = format!("{error:#}"),
                bot_type,
                app_id = %app_id,
                track_namespace_prefix = prefix,
                "bot launch failed"
            );
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        }
    };
    tracing::info!(
        error,
        bot_type,
        app_id = %app_id,
        track_namespace_prefix = prefix,
        "bot launch refused"
    );
    error_response(status, error)
}

async fn latest(
    state: &ServerState,
    headers: &HeaderMap,
    bot_type: &str,
    query: Option<&str>,
) -> JsonResponse {
    let prefix = match authorized_prefix(state, headers, query).await {
        Ok(prefix) => prefix,
        Err((status, error)) => return error_response(status, error),
    };
    match state.bots.latest(bot_type, &prefix, now_ms()).await {
        Ok(Some(bot)) => json_response(StatusCode::OK, &bot),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "not_found"),
        Err(error) => internal(&error),
    }
}

async fn room(state: &ServerState, headers: &HeaderMap, query: Option<&str>) -> JsonResponse {
    let prefix = match authorized_prefix(state, headers, query).await {
        Ok(prefix) => prefix,
        Err((status, error)) => return error_response(status, error),
    };
    match state.bots.room(&prefix, now_ms()).await {
        Ok(bots) => json_response(StatusCode::OK, &json!({ "bots": bots })),
        Err(error) => internal(&error),
    }
}

/// The query's `namespace_prefix`, when the launch token covers it;
/// otherwise the refusal, already logged.
async fn authorized_prefix(
    state: &ServerState,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<String, (StatusCode, &'static str)> {
    let Some(prefix) = query_params(query).remove("namespace_prefix") else {
        return Err((StatusCode::BAD_REQUEST, "invalid_request"));
    };
    match state.launch_auth.authorize(headers, &prefix).await {
        Ok(_) => Ok(prefix),
        Err(rejection) => {
            log_refusal(&rejection, &prefix);
            Err((rejection.status, rejection.error))
        }
    }
}

async fn show(state: &ServerState, headers: &HeaderMap, bot_type: &str, id: &str) -> JsonResponse {
    match authorized_record(state, headers, bot_type, id).await {
        Ok(record) => match state.bots.view(&record, now_ms()).await {
            Ok(bot) => json_response(StatusCode::OK, &bot),
            Err(error) => internal(&error),
        },
        Err((status, error)) => error_response(status, error),
    }
}

async fn stop(state: &ServerState, headers: &HeaderMap, bot_type: &str, id: &str) -> JsonResponse {
    let record = match authorized_record(state, headers, bot_type, id).await {
        Ok(record) => record,
        Err((status, error)) => return error_response(status, error),
    };
    match state.bots.stop(id).await {
        Ok(()) => {
            tracing::info!(
                bot_id = id,
                bot_type = %record.bot_type,
                app_id = %record.app_id,
                track_namespace_prefix = %record.namespace_prefix,
                "bot stop requested"
            );
            empty_response(StatusCode::ACCEPTED)
        }
        Err(error) => internal(&error),
    }
}

/// The bot, when the launch token covers its room; otherwise the refusal,
/// already logged.
async fn authorized_record(
    state: &ServerState,
    headers: &HeaderMap,
    bot_type: &str,
    id: &str,
) -> Result<BotRecord, (StatusCode, &'static str)> {
    let record = match state.bots.record(bot_type, id).await {
        Ok(Some(record)) => record,
        Ok(None) => return Err((StatusCode::NOT_FOUND, "not_found")),
        Err(error) => {
            tracing::error!(
                error = format!("{error:#}"),
                "bot-dispatcher request failed"
            );
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "internal_error"));
        }
    };
    match state
        .launch_auth
        .authorize(headers, &record.namespace_prefix)
        .await
    {
        Ok(_) => Ok(record),
        Err(rejection) => {
            log_refusal(&rejection, &record.namespace_prefix);
            Err((rejection.status, rejection.error))
        }
    }
}

async fn token(state: &ServerState, headers: &HeaderMap, bot_type: &str, id: &str) -> JsonResponse {
    match state
        .bots
        .token(bot_type, id, bearer_token(headers), now_ms())
        .await
    {
        Ok(token) => {
            tracing::info!(bot_id = id, "bot token issued");
            json_response(StatusCode::OK, &json!({ "token": token }))
        }
        Err(error) => bot_error(error, id),
    }
}

async fn heartbeat(
    state: &ServerState,
    headers: &HeaderMap,
    bot_type: &str,
    id: &str,
    body: Incoming,
) -> JsonResponse {
    let Some(heartbeat) = read_json::<Heartbeat, _>(body).await else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let reported = heartbeat.state;
    match state
        .bots
        .heartbeat(bot_type, id, bearer_token(headers), heartbeat, now_ms())
        .await
    {
        Ok(stop) => {
            if reported.has_ended() {
                tracing::info!(bot_id = id, bot_state = ?reported, "bot ended");
            } else {
                tracing::debug!(bot_id = id, bot_state = ?reported, stop, "bot heartbeat");
            }
            json_response(StatusCode::OK, &json!({ "stop": stop }))
        }
        Err(error) => bot_error(error, id),
    }
}

fn bot_error(error: BotError, id: &str) -> JsonResponse {
    let (status, error) = match error {
        BotError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        BotError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
        BotError::Ended => (StatusCode::GONE, "ended"),
        BotError::Internal(error) => return internal(&error),
    };
    tracing::info!(bot_id = id, error, "bot request refused");
    error_response(status, error)
}

fn refused(rejection: Rejection, prefix: &str) -> JsonResponse {
    log_refusal(&rejection, prefix);
    error_response(rejection.status, rejection.error)
}

fn log_refusal(rejection: &Rejection, prefix: &str) {
    tracing::info!(
        error = rejection.error,
        track_namespace_prefix = prefix,
        "launch token refused"
    );
}

fn internal(error: &anyhow::Error) -> JsonResponse {
    tracing::error!(
        error = format!("{error:#}"),
        "bot-dispatcher request failed"
    );
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is after the Unix epoch")
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use http_server::testing::StubServer;
    use serde_json::Value;

    use super::*;
    use crate::test_support::{launch_token_answer, test_bots};

    /// A server over `test_bots()` whose VTS accepts every bearer as the app
    /// `app`'s launch token for its whole root, and its base URL.
    async fn start_server() -> (String, BotDispatcherServer, StubServer) {
        let vts = StubServer::run(StatusCode::OK, launch_token_answer()).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = BotDispatcherServer::run(
            listener,
            test_bots().bots,
            LaunchAuth::new(format!("{}/verify", vts.url())),
        );
        (base, server, vts)
    }

    #[tokio::test]
    async fn an_app_server_starts_finds_and_stops_a_bot_of_a_type() {
        // Arrange
        let (base, _server, _vts) = start_server().await;
        let client = reqwest::Client::new();

        // Act
        let started: Value = client
            .post(format!("{base}/bots/recorder"))
            .bearer_auth("launch-token")
            .json(&json!({ "namespace_prefix": "app/room" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = started["id"].as_str().unwrap();
        let latest: Value = client
            .get(format!("{base}/bots/recorder?namespace_prefix=app%2Froom"))
            .bearer_auth("launch-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let shown: Value = client
            .get(format!("{base}/bots/recorder/{id}"))
            .bearer_auth("launch-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let unknown_type = client
            .post(format!("{base}/bots/chat"))
            .bearer_auth("launch-token")
            .json(&json!({ "namespace_prefix": "app/room" }))
            .send()
            .await
            .unwrap()
            .status();
        let stopped = client
            .delete(format!("{base}/bots/recorder/{id}"))
            .bearer_auth("launch-token")
            .send()
            .await
            .unwrap()
            .status();
        let unauthenticated = client
            .post(format!("{base}/bots/recorder"))
            .json(&json!({ "namespace_prefix": "app/other" }))
            .send()
            .await
            .unwrap()
            .status();
        let launch_token_as_bot = client
            .post(format!("{base}/bots/recorder/{id}/token"))
            .bearer_auth("launch-token")
            .send()
            .await
            .unwrap()
            .status();

        // Assert
        assert_eq!(started["state"], "starting");
        assert_eq!(latest["id"], id);
        assert_eq!(shown["id"], id);
        assert_eq!(unknown_type, StatusCode::NOT_FOUND);
        assert_eq!(stopped, StatusCode::ACCEPTED);
        assert_eq!(unauthenticated, StatusCode::UNAUTHORIZED);
        assert_eq!(launch_token_as_bot, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn an_app_server_lists_the_bots_of_a_room_it_covers() {
        // Arrange
        let (base, _server, _vts) = start_server().await;
        let client = reqwest::Client::new();
        let started: Value = client
            .post(format!("{base}/bots/recorder"))
            .bearer_auth("launch-token")
            .json(&json!({ "namespace_prefix": "app/room" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let get = |path: &str| {
            client
                .get(format!("{base}{path}"))
                .bearer_auth("launch-token")
        };

        // Act
        let listed: Value = get("/bots?namespace_prefix=app%2Froom")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let empty: Value = get("/bots?namespace_prefix=app%2Fquiet")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let without_prefix = get("/bots").send().await.unwrap().status();
        let other_app = get("/bots?namespace_prefix=other%2Froom")
            .send()
            .await
            .unwrap()
            .status();
        let unauthenticated = client
            .get(format!("{base}/bots?namespace_prefix=app%2Froom"))
            .send()
            .await
            .unwrap()
            .status();

        // Assert
        assert_eq!(listed["bots"].as_array().unwrap().len(), 1);
        assert_eq!(listed["bots"][0]["id"], started["id"]);
        assert_eq!(listed["bots"][0]["type"], "recorder");
        assert_eq!(empty, json!({ "bots": [] }));
        assert_eq!(without_prefix, StatusCode::BAD_REQUEST);
        assert_eq!(other_app, StatusCode::FORBIDDEN);
        assert_eq!(unauthenticated, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_launch_with_options_its_type_refuses_is_a_bad_request() {
        // Arrange
        let (base, _server, _vts) = start_server().await;
        let client = reqwest::Client::new();
        let launch = |options: Value| {
            client
                .post(format!("{base}/bots/recorder"))
                .bearer_auth("launch-token")
                .json(&json!({ "namespace_prefix": "app/room", "options": options }))
                .send()
        };

        // Act
        let refused = launch(json!({ "format": "webm" })).await.unwrap();
        let refused_status = refused.status();
        let refused_body: Value = refused.json().await.unwrap();
        let null_options = launch(Value::Null).await.unwrap().status();

        // Assert
        assert_eq!(refused_status, StatusCode::BAD_REQUEST);
        assert_eq!(refused_body, json!({ "error": "invalid_options" }));
        assert_eq!(null_options, StatusCode::CREATED);
    }

    #[tokio::test]
    async fn a_launch_whose_stay_when_empty_is_not_a_boolean_is_an_invalid_request() {
        // Arrange
        let (base, _server, _vts) = start_server().await;

        // Act
        let response = reqwest::Client::new()
            .post(format!("{base}/bots/recorder"))
            .bearer_auth("launch-token")
            .json(&json!({ "namespace_prefix": "app/room", "stay_when_empty": "yes" }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();

        // Assert
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({ "error": "invalid_request" }));
    }
}

use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use http_body_util::Full;
use http_server::{
    HttpServer, bearer_token, empty_response, error_response, json_response, query_params,
    read_json,
};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tracing::Instrument;

use crate::{
    bots::{BotError, Bots, Heartbeat, StartError},
    launch_auth::{LaunchAuth, Rejection},
    store::BotRecord,
};

type JsonResponse = Response<Full<Bytes>>;

pub(crate) struct BotsServer {
    _http: HttpServer,
}

struct ServerState {
    bots: Bots,
    launch_auth: LaunchAuth,
}

#[derive(Deserialize)]
struct StartRequest {
    namespace_prefix: String,
    #[serde(default)]
    options: Option<Value>,
}

impl BotsServer {
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
        "bots.request",
        "http.request.method" = %head.method,
        "url.path" = head.uri.path(),
        "http.response.status_code" = tracing::field::Empty,
    );
    telemetry_otlp::continue_trace_from(&span, &head.headers);
    let path = head.uri.path().to_string();
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let response = async {
        match (&head.method, segments.as_slice()) {
            (&Method::GET, ["healthz"]) => {
                json_response(StatusCode::OK, &json!({ "status": "ok" }))
            }
            (&Method::POST, ["bots", bot_type]) => {
                start(&state, &head.headers, bot_type, body).await
            }
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
        .start(
            &app_id,
            bot_type,
            prefix,
            request.options.as_ref(),
            now_ms(),
        )
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
    let Some(prefix) = query_params(query).remove("namespace_prefix") else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if let Err(rejection) = state.launch_auth.authorize(headers, &prefix).await {
        return refused(rejection, &prefix);
    }
    match state.bots.latest(bot_type, &prefix, now_ms()).await {
        Ok(Some(bot)) => json_response(StatusCode::OK, &bot),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "not_found"),
        Err(error) => internal(&error),
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
            tracing::error!(error = format!("{error:#}"), "bots request failed");
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
    tracing::error!(error = format!("{error:#}"), "bots request failed");
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
    use serde_json::Value;

    use super::*;
    use crate::test_support::{fake_vts, test_bots};

    #[tokio::test]
    async fn an_app_server_starts_finds_and_stops_a_bot_of_a_type() {
        // Arrange
        let vts = fake_vts().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let _server = BotsServer::run(listener, test_bots().bots, LaunchAuth::new(vts.url.clone()));
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
}

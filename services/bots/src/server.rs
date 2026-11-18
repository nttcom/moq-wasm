use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use http_body_util::Full;
use http_server::{HttpServer, empty_response, error_response, json_response, read_json};
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
    launch_auth::{LaunchAuth, Rejection, bearer_token},
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
    #[serde(rename = "type")]
    bot_type: String,
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
            (&Method::POST, ["bots"]) => start(&state, &head.headers, body).await,
            (&Method::GET, ["bots"]) => list(&state, &head.headers, head.uri.query()).await,
            (&Method::DELETE, ["bots", id]) => stop(&state, &head.headers, id).await,
            (&Method::POST, ["bots", id, "token"]) => token(&state, &head.headers, id).await,
            (&Method::POST, ["bots", id, "heartbeat"]) => {
                heartbeat(&state, &head.headers, id, body).await
            }
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    response
}

async fn start(state: &ServerState, headers: &HeaderMap, body: Incoming) -> JsonResponse {
    let Some(request) = read_json::<StartRequest, _>(body).await else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let prefix = request.namespace_prefix.as_str();
    let launch = match state.launch_auth.authorize(headers, prefix).await {
        Ok(launch) => launch,
        Err(rejection) => return refused(rejection, prefix),
    };
    let result = state
        .bots
        .start(
            &launch.app_id,
            &request.bot_type,
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
                app_id = %launch.app_id,
                track_namespace_prefix = prefix,
                "bot launched"
            );
            return json_response(StatusCode::CREATED, &bot);
        }
        Err(StartError::UnknownType) => (StatusCode::BAD_REQUEST, "unknown_type"),
        Err(StartError::InvalidRoom) => (StatusCode::BAD_REQUEST, "invalid_room"),
        Err(StartError::Conflict) => (StatusCode::CONFLICT, "conflict"),
        Err(StartError::Full) => (StatusCode::SERVICE_UNAVAILABLE, "full"),
        Err(StartError::Internal(error)) => {
            tracing::error!(
                error = format!("{error:#}"),
                bot_type = %request.bot_type,
                app_id = %launch.app_id,
                track_namespace_prefix = prefix,
                "bot launch failed"
            );
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error");
        }
    };
    tracing::info!(
        error,
        bot_type = %request.bot_type,
        app_id = %launch.app_id,
        track_namespace_prefix = prefix,
        "bot launch refused"
    );
    error_response(status, error)
}

async fn list(state: &ServerState, headers: &HeaderMap, query: Option<&str>) -> JsonResponse {
    let Some(prefix) = query.and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "namespace_prefix")
            .map(|(_, value)| value.into_owned())
    }) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    if let Err(rejection) = state.launch_auth.authorize(headers, &prefix).await {
        return refused(rejection, &prefix);
    }
    match state.bots.list(&prefix, now_ms()).await {
        Ok(bots) => json_response(StatusCode::OK, &json!({ "bots": bots })),
        Err(error) => internal(&error),
    }
}

async fn stop(state: &ServerState, headers: &HeaderMap, id: &str) -> JsonResponse {
    let record = match state.bots.record(id).await {
        Ok(Some(record)) => record,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "not_found"),
        Err(error) => return internal(&error),
    };
    if let Err(rejection) = state
        .launch_auth
        .authorize(headers, &record.namespace_prefix)
        .await
    {
        return refused(rejection, &record.namespace_prefix);
    }
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

async fn token(state: &ServerState, headers: &HeaderMap, id: &str) -> JsonResponse {
    match state.bots.token(id, bearer_token(headers), now_ms()).await {
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
    id: &str,
    body: Incoming,
) -> JsonResponse {
    let Some(heartbeat) = read_json::<Heartbeat, _>(body).await else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let reported = heartbeat.state;
    match state
        .bots
        .heartbeat(id, bearer_token(headers), heartbeat, now_ms())
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
    tracing::info!(
        error = rejection.error,
        track_namespace_prefix = prefix,
        "launch token refused"
    );
    error_response(rejection.status, rejection.error)
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
    async fn an_app_server_starts_lists_and_stops_a_bot() {
        // Arrange
        let vts = fake_vts().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let _server = BotsServer::run(listener, test_bots().bots, LaunchAuth::new(vts.url.clone()));
        let client = reqwest::Client::new();

        // Act
        let started: Value = client
            .post(format!("{base}/bots"))
            .bearer_auth("launch-token")
            .json(&json!({ "type": "recorder", "namespace_prefix": "app/room" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = started["id"].as_str().unwrap();
        let listed: Value = client
            .get(format!("{base}/bots?namespace_prefix=app%2Froom"))
            .bearer_auth("launch-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let stopped = client
            .delete(format!("{base}/bots/{id}"))
            .bearer_auth("launch-token")
            .send()
            .await
            .unwrap()
            .status();
        let unauthenticated = client
            .post(format!("{base}/bots"))
            .json(&json!({ "type": "recorder", "namespace_prefix": "app/other" }))
            .send()
            .await
            .unwrap()
            .status();
        let launch_token_as_bot = client
            .post(format!("{base}/bots/{id}/token"))
            .bearer_auth("launch-token")
            .send()
            .await
            .unwrap()
            .status();

        // Assert
        assert_eq!(started["state"], "starting");
        assert_eq!(listed["bots"][0]["id"], id);
        assert_eq!(stopped, StatusCode::ACCEPTED);
        assert_eq!(unauthenticated, StatusCode::UNAUTHORIZED);
        assert_eq!(launch_token_as_bot, StatusCode::UNAUTHORIZED);
    }
}

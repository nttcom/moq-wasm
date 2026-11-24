use std::sync::Arc;

use http_body_util::Full;
use http_server::{HttpServer, error_response, json_response, read_json};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
};
use serde_json::json;
use tokio::net::TcpListener;
use tracing::Instrument;

use crate::{
    call_auth::CallAuth,
    calls::{CallRequest, Calls, PlaceError},
};

type JsonResponse = Response<Full<Bytes>>;

/// The HTTP API app servers place calls with.
pub(crate) struct GatewayServer {
    _http: HttpServer,
}

struct ServerState {
    calls: Arc<Calls>,
    call_auth: CallAuth,
}

impl GatewayServer {
    pub(crate) fn run(listener: TcpListener, calls: Arc<Calls>, call_auth: CallAuth) -> Self {
        let state = Arc::new(ServerState { calls, call_auth });
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
        "sip_gateway.request",
        "http.request.method" = %head.method,
        "url.path" = head.uri.path(),
        "http.response.status_code" = tracing::field::Empty,
    );
    telemetry_otlp::continue_trace_from(&span, &head.headers);
    let response = async {
        match (&head.method, head.uri.path()) {
            (&Method::GET, "/healthz") => json_response(StatusCode::OK, &json!({ "status": "ok" })),
            (&Method::POST, "/calls") => place(&state, &head.headers, body).await,
            _ => error_response(StatusCode::NOT_FOUND, "not_found"),
        }
    }
    .instrument(span.clone())
    .await;
    span.record("http.response.status_code", response.status().as_u16());
    response
}

async fn place(state: &ServerState, headers: &HeaderMap, body: Incoming) -> JsonResponse {
    let Some(request) = read_json::<CallRequest, _>(body).await else {
        return error_response(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let prefix = request.namespace_prefix.as_str();
    let authorized = match state
        .call_auth
        .authorize(headers, prefix, &request.to)
        .await
    {
        Ok(authorized) => authorized,
        Err(rejection) => {
            tracing::warn!(
                track_namespace_prefix = prefix,
                error = rejection.error,
                "call refused"
            );
            return error_response(rejection.status, rejection.error);
        }
    };
    let app_id = authorized.app_id.clone();
    let placed = state.calls.place(authorized, request);
    match placed {
        Ok(placed) => {
            tracing::info!(
                call_id = placed.call_id,
                app_id,
                direction = "outbound",
                "call placed"
            );
            json_response(
                StatusCode::CREATED,
                &json!({ "call_id": placed.call_id, "namespace": placed.namespace }),
            )
        }
        Err(PlaceError::InvalidPeer) => error_response(StatusCode::BAD_REQUEST, "invalid_peer"),
        Err(PlaceError::Busy) => error_response(StatusCode::TOO_MANY_REQUESTS, "too_many_calls"),
        Err(PlaceError::Failed(error)) => {
            tracing::error!(%error, app_id, "call not placed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
        }
    }
}

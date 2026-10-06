use std::{collections::HashMap, convert::Infallible, sync::Arc};

use anyhow::Context;
use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_TYPE, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::json;
use tokio::{net::TcpListener, task::JoinHandle};

use crate::{
    browser_json::snapshots_for_browser,
    clickhouse::ClickHouse,
    latest_snapshots::LatestSnapshots,
    series::{self, SeriesQuery, SeriesTarget},
    snapshot_history::snapshots_at,
};

const DEFAULT_POINTS: u64 = 90;

pub struct ApiState {
    pub latest: LatestSnapshots,
    pub clickhouse: ClickHouse,
}

pub struct ApiServer {
    _accept_handle: JoinHandle<()>,
}

impl ApiServer {
    pub fn run(listener: TcpListener, state: ApiState) -> Self {
        Self {
            _accept_handle: tokio::spawn(accept_connections(listener, Arc::new(state))),
        }
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
        let state = state.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| handle(request, state.clone()));
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

fn query_params(request: &Request<Incoming>) -> HashMap<String, String> {
    request
        .uri()
        .query()
        .map(|query| {
            reqwest::Url::parse(&format!("http://query/?{query}"))
                .map(|url| url.query_pairs().into_owned().collect())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

async fn handle(
    request: Request<Incoming>,
    state: Arc<ApiState>,
) -> Result<JsonResponse, Infallible> {
    let params = query_params(&request);
    let response = match (request.method(), request.uri().path()) {
        (&Method::GET, "/api/snapshots") => snapshots(&state, &params).await,
        (&Method::GET, "/api/series") => series(&state, &params).await,
        (&Method::GET, "/healthz") => Ok(json_response(StatusCode::OK, &json!({ "status": "ok" }))),
        _ => Ok(error_response(StatusCode::NOT_FOUND, "not_found")),
    };
    Ok(response.unwrap_or_else(|error| {
        tracing::warn!(?error, "request failed");
        error_response(StatusCode::BAD_REQUEST, &format!("{error:#}"))
    }))
}

fn required<'a>(params: &'a HashMap<String, String>, name: &str) -> anyhow::Result<&'a str> {
    params
        .get(name)
        .map(String::as_str)
        .with_context(|| format!("missing query parameter {name}"))
}

fn number(params: &HashMap<String, String>, name: &str) -> anyhow::Result<u64> {
    required(params, name)?
        .parse()
        .with_context(|| format!("query parameter {name} is not a number"))
}

async fn snapshots(
    state: &ApiState,
    params: &HashMap<String, String>,
) -> anyhow::Result<JsonResponse> {
    let snapshots = match params.get("at") {
        None => state.latest.all(),
        Some(_) => snapshots_at(&state.clickhouse, number(params, "at")?).await?,
    };
    Ok(json_response(
        StatusCode::OK,
        &snapshots_for_browser(&snapshots),
    ))
}

pub fn series_query(params: &HashMap<String, String>) -> anyhow::Result<SeriesQuery> {
    let target = match required(params, "target")? {
        "process" => SeriesTarget::Process,
        "relay" => SeriesTarget::Relay,
        "session" => SeriesTarget::Session {
            session_id: number(params, "session_id")?,
        },
        "track" => SeriesTarget::Track {
            publisher_session_id: number(params, "publisher_session_id")?,
            namespace: required(params, "namespace")?.to_string(),
            name: required(params, "name")?.to_string(),
        },
        "subscription" => SeriesTarget::Subscription {
            subscriber_session_id: number(params, "subscriber_session_id")?,
            request_id: number(params, "request_id")?,
        },
        other => anyhow::bail!("unknown series target {other:?}"),
    };
    Ok(SeriesQuery {
        relay_id: required(params, "relay_id")?.to_string(),
        target,
        from_ms: number(params, "from")?,
        to_ms: number(params, "to")?,
        points: params
            .get("points")
            .map(|points| points.parse())
            .transpose()
            .context("query parameter points is not a number")?
            .unwrap_or(DEFAULT_POINTS),
    })
}

async fn series(
    state: &ApiState,
    params: &HashMap<String, String>,
) -> anyhow::Result<JsonResponse> {
    let query = series_query(params)?;
    let series = series::query(&state.clickhouse, &query).await?;
    Ok(json_response(StatusCode::OK, &series))
}

fn error_response(status: StatusCode, error: &str) -> JsonResponse {
    json_response(status, &json!({ "error": error }))
}

fn json_response(status: StatusCode, body: &impl Serialize) -> JsonResponse {
    let body = serde_json::to_vec(body).unwrap_or_else(|_| b"null".to_vec());
    let mut response = Response::new(Full::from(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::series_query;
    use crate::series::SeriesTarget;

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_track_series_is_identified_by_its_publisher_and_full_name() {
        // Arrange
        let params = params(&[
            ("relay_id", "relay-a"),
            ("target", "track"),
            ("publisher_session_id", "7"),
            ("namespace", "app/live"),
            ("name", "video"),
            ("from", "0"),
            ("to", "60000"),
        ]);

        // Act
        let query = series_query(&params).unwrap();

        // Assert
        assert_eq!(
            query.target,
            SeriesTarget::Track {
                publisher_session_id: 7,
                namespace: "app/live".to_string(),
                name: "video".to_string(),
            }
        );
        assert_eq!(query.points, 90);
    }

    #[test]
    fn a_session_series_without_its_id_is_rejected() {
        // Arrange
        let params = params(&[
            ("relay_id", "relay-a"),
            ("target", "session"),
            ("from", "0"),
            ("to", "1"),
        ]);

        // Act / Assert
        assert!(series_query(&params).is_err());
    }
}

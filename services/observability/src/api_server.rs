use std::{collections::HashMap, net::IpAddr, sync::Arc};

use anyhow::Context;
use http_body_util::Full;
use http_server::{HttpServer, error_response, json_response};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{
        ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
        ACCESS_CONTROL_MAX_AGE, HeaderValue,
    },
};
use serde_json::json;
use tokio::net::TcpListener;

use crate::{
    app_verifier::AppVerifier,
    browser_json::snapshots_for_browser,
    clickhouse::ClickHouse,
    client_telemetry::ClientTelemetry,
    namespaces::{NamespaceQuery, published_namespaces},
    schema::RETENTION_DAYS,
    series::{self, SeriesQuery, SeriesTarget},
    snapshot_history::{RELAY_SILENCE, latest_snapshots, snapshots_at},
    usage::{UsageQuery, usage},
    viewer::viewer_app,
};

const DEFAULT_POINTS: u64 = 90;
const MAX_POINTS: u64 = 300;
const MAX_SPAN_MS: u64 = RETENTION_DAYS as u64 * 86_400_000;

pub struct ApiState {
    pub clickhouse: ClickHouse,
    /// `None` answers only requests without a token, which read `anon`.
    pub verifier: Option<Arc<AppVerifier>>,
    /// `None` answers `POST /api/client-telemetry` with 404.
    pub client_telemetry: Option<ClientTelemetry>,
}

pub struct ApiServer {
    _http: HttpServer,
}

impl ApiServer {
    pub fn run(listener: TcpListener, state: ApiState) -> Self {
        let state = Arc::new(state);
        Self {
            _http: HttpServer::run(listener, move |request, peer| {
                handle(request, peer.ip(), state.clone())
            }),
        }
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

async fn handle(request: Request<Incoming>, peer: IpAddr, state: Arc<ApiState>) -> JsonResponse {
    let mut response = route(request, peer, &state).await;
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}

async fn route(request: Request<Incoming>, peer: IpAddr, state: &ApiState) -> JsonResponse {
    match (request.method(), request.uri().path()) {
        (&Method::POST, "/api/client-telemetry") => {
            return client_telemetry(request, peer, state).await;
        }
        (&Method::OPTIONS, _) => return preflight(),
        (&Method::GET, "/healthz") => {
            return json_response(StatusCode::OK, &json!({ "status": "ok" }));
        }
        (&Method::GET, "/api/snapshots" | "/api/series" | "/api/namespaces" | "/api/usage") => {}
        _ => return error_response(StatusCode::NOT_FOUND, "not_found"),
    }
    let app_id = match viewer_app(request.headers(), state.verifier.as_deref()).await {
        Ok(app_id) => app_id,
        Err(refusal) => {
            let (status, code) = refusal.response();
            return error_response(status, code);
        }
    };
    let params = query_params(&request);
    let response = match request.uri().path() {
        "/api/snapshots" => snapshots(state, &params, &app_id).await,
        "/api/series" => series(state, &params, app_id).await,
        "/api/usage" => app_usage(state, &params, app_id).await,
        _ => namespaces(state, &params, app_id).await,
    };
    response.unwrap_or_else(|error| {
        tracing::warn!(?error, "request failed");
        error_response(StatusCode::BAD_REQUEST, &format!("{error:#}"))
    })
}

/// A page sends its token in `Authorization`, which needs a CORS preflight.
fn preflight() -> JsonResponse {
    let mut response = Response::new(Full::new(Bytes::new()));
    *response.status_mut() = StatusCode::NO_CONTENT;
    let headers = response.headers_mut();
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST"),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("authorization"),
    );
    headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
    response
}

/// Browsers post as `text/plain`, a CORS simple request, so there is no preflight to answer.
async fn client_telemetry(
    request: Request<Incoming>,
    peer: IpAddr,
    state: &ApiState,
) -> JsonResponse {
    let Some(client_telemetry) = &state.client_telemetry else {
        return error_response(StatusCode::NOT_FOUND, "not_found");
    };
    let (parts, body) = request.into_parts();
    match client_telemetry.receive(peer, &parts.headers, body).await {
        Ok(()) => json_response(StatusCode::ACCEPTED, &json!({ "status": "accepted" })),
        Err(rejection) => error_response(rejection.status(), &rejection.code()),
    }
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
    app_id: &str,
) -> anyhow::Result<JsonResponse> {
    let body = match past_snapshot_times(params)? {
        None => snapshots_for_browser(&latest_snapshots(&state.clickhouse).await?, app_id),
        Some((at_ms, previous_ms)) => {
            let past = snapshots_at(&state.clickhouse, at_ms, previous_ms).await?;
            json!({
                "snapshots": snapshots_for_browser(&past.snapshots, app_id),
                "previous": snapshots_for_browser(&past.previous, app_id),
            })
        }
    };
    Ok(json_response(StatusCode::OK, &body))
}

/// `at` and `previous` of a past lookup; `None` asks for the live snapshots.
fn past_snapshot_times(params: &HashMap<String, String>) -> anyhow::Result<Option<(u64, u64)>> {
    if !params.contains_key("at") {
        return Ok(None);
    }
    let at_ms = number(params, "at")?;
    let previous_ms = number(params, "previous")?;
    anyhow::ensure!(previous_ms <= at_ms, "previous is after at");
    anyhow::ensure!(
        at_ms - previous_ms <= RELAY_SILENCE.as_millis() as u64,
        "previous is at most {} s before at",
        RELAY_SILENCE.as_secs()
    );
    Ok(Some((at_ms, previous_ms)))
}

fn time_range(params: &HashMap<String, String>) -> anyhow::Result<(u64, u64)> {
    let from_ms = number(params, "from")?;
    let to_ms = number(params, "to")?;
    anyhow::ensure!(from_ms <= to_ms, "from is after to");
    anyhow::ensure!(
        to_ms - from_ms <= MAX_SPAN_MS,
        "a range spans at most {RETENTION_DAYS} days"
    );
    Ok((from_ms, to_ms))
}

async fn namespaces(
    state: &ApiState,
    params: &HashMap<String, String>,
    app_id: String,
) -> anyhow::Result<JsonResponse> {
    let (from_ms, to_ms) = time_range(params)?;
    let query = NamespaceQuery {
        from_ms,
        to_ms,
        app_id,
    };
    let namespaces = published_namespaces(&state.clickhouse, &query).await?;
    Ok(json_response(StatusCode::OK, &namespaces))
}

pub fn series_query(
    params: &HashMap<String, String>,
    app_id: String,
) -> anyhow::Result<SeriesQuery> {
    let target = match required(params, "target")? {
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
    let (from_ms, to_ms) = time_range(params)?;
    let points = points(params)?;
    Ok(SeriesQuery {
        app_id,
        relay_id: Some(required(params, "relay_id")?.to_string()),
        target,
        from_ms,
        to_ms,
        points,
    })
}

async fn series(
    state: &ApiState,
    params: &HashMap<String, String>,
    app_id: String,
) -> anyhow::Result<JsonResponse> {
    let query = series_query(params, app_id)?;
    let series = series::query(&state.clickhouse, &query).await?;
    Ok(json_response(StatusCode::OK, &series))
}

fn points(params: &HashMap<String, String>) -> anyhow::Result<u64> {
    let points = params
        .get("points")
        .map(|points| points.parse())
        .transpose()
        .context("query parameter points is not a number")?
        .unwrap_or(DEFAULT_POINTS);
    anyhow::ensure!(
        (1..=MAX_POINTS).contains(&points),
        "points must be between 1 and {MAX_POINTS}"
    );
    Ok(points)
}

fn usage_query(params: &HashMap<String, String>, app_id: String) -> anyhow::Result<UsageQuery> {
    let (from_ms, to_ms) = time_range(params)?;
    Ok(UsageQuery {
        app_id,
        from_ms,
        to_ms,
        points: points(params)?,
    })
}

async fn app_usage(
    state: &ApiState,
    params: &HashMap<String, String>,
    app_id: String,
) -> anyhow::Result<JsonResponse> {
    let query = usage_query(params, app_id)?;
    let usage = usage(&state.clickhouse, &query).await?;
    Ok(json_response(StatusCode::OK, &usage))
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use hyper::StatusCode;

    use super::{ApiState, past_snapshot_times, series_query};
    use crate::{
        app_verifier::AppVerifier,
        series::{SeriesQuery, SeriesTarget},
        test_support::{api_server, fake_http_server_answering, vts_answer},
    };

    async fn get(url: &str, token: Option<&str>) -> StatusCode {
        let mut request = reqwest::Client::new().get(url);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send().await.unwrap().status()
    }

    #[tokio::test]
    async fn a_stats_request_with_a_token_short_of_its_whole_app_reads_nothing() {
        // Arrange
        let (vts_url, _vts_requests) =
            fake_http_server_answering(StatusCode::OK, &vts_answer("app-1", Some("room"))).await;
        let verifier = Arc::new(AppVerifier::new(format!("{vts_url}/verify")).unwrap());
        let (url, _api) = api_server(|clickhouse| ApiState {
            clickhouse,
            verifier: Some(verifier),
            client_telemetry: None,
        })
        .await;

        // Act / Assert
        for path in [
            "/api/snapshots",
            "/api/series",
            "/api/namespaces",
            "/api/usage",
        ] {
            assert_eq!(
                get(&format!("{url}{path}"), Some("jwt")).await,
                StatusCode::FORBIDDEN
            );
        }
    }

    #[tokio::test]
    async fn a_page_may_send_its_token_from_any_origin() {
        // Arrange
        let (url, _api) = api_server(|clickhouse| ApiState {
            clickhouse,
            verifier: None,
            client_telemetry: None,
        })
        .await;

        // Act
        let response = reqwest::Client::new()
            .request(reqwest::Method::OPTIONS, format!("{url}/api/snapshots"))
            .header("origin", "https://example.com")
            .header("access-control-request-headers", "authorization")
            .send()
            .await
            .unwrap();

        // Assert
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let headers = response.headers();
        assert_eq!(headers["access-control-allow-origin"], "*");
        assert_eq!(headers["access-control-allow-headers"], "authorization");
    }

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
        let query = series_query(&params, "app".to_string()).unwrap();

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

    fn relay_series(from: &str, to: &str, points: &str) -> anyhow::Result<SeriesQuery> {
        series_query(
            &params(&[
                ("relay_id", "relay-a"),
                ("target", "relay"),
                ("from", from),
                ("to", to),
                ("points", points),
            ]),
            "app".to_string(),
        )
    }

    #[test]
    fn a_series_longer_than_the_retention_is_rejected() {
        // Act / Assert
        assert!(relay_series("0", "604800000", "90").is_ok());
        assert!(relay_series("0", "604800001", "90").is_err());
    }

    #[test]
    fn more_points_than_the_maximum_are_rejected() {
        // Act / Assert
        assert!(relay_series("0", "60000", "300").is_ok());
        assert!(relay_series("0", "60000", "301").is_err());
        assert!(relay_series("0", "60000", "0").is_err());
    }

    #[test]
    fn a_range_ending_before_it_starts_is_rejected() {
        // Act / Assert
        assert!(relay_series("60000", "0", "90").is_err());
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
        assert!(series_query(&params, "app".to_string()).is_err());
    }

    #[test]
    fn a_past_lookup_needs_a_previous_time_within_the_relay_silence() {
        // Act / Assert
        assert_eq!(past_snapshot_times(&params(&[])).unwrap(), None);
        assert_eq!(
            past_snapshot_times(&params(&[("at", "60000"), ("previous", "58000")])).unwrap(),
            Some((60000, 58000))
        );
        assert!(past_snapshot_times(&params(&[("at", "60000")])).is_err());
        assert!(past_snapshot_times(&params(&[("at", "60000"), ("previous", "60001")])).is_err());
        assert!(past_snapshot_times(&params(&[("at", "60000"), ("previous", "49999")])).is_err());
    }
}

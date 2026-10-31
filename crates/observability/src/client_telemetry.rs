mod app_verifier;
mod otlp_export;
mod rate_limiter;
mod report;

use std::net::IpAddr;

use http_body_util::{BodyExt, LengthLimitError, Limited};
use hyper::{HeaderMap, StatusCode, body::Incoming};

use app_verifier::{AppVerifier, VerifyError};
use rate_limiter::{RateLimiter, client_address};
use report::ClientReport;

use crate::otlp;

const MAX_BODY_BYTES: usize = 64 * 1024;
/// The app of a report without a token, as the relay names an anonymous session's app.
const ANONYMOUS_APP_ID: &str = "anon";

pub struct ClientTelemetryConfig {
    /// The VTS `…/verify` URL; without it, every report is anonymous.
    pub vts_url: Option<String>,
    pub otlp_logs_endpoint: Option<String>,
    pub otlp_metrics_endpoint: Option<String>,
}

/// Receives the logs and player stats browsers post and forwards them to the
/// collector as OTLP.
pub struct ClientTelemetry {
    http: reqwest::Client,
    verifier: Option<AppVerifier>,
    otlp_logs_endpoint: Option<String>,
    otlp_metrics_endpoint: Option<String>,
    rate_limiter: RateLimiter,
}

#[derive(Debug)]
pub(crate) enum Rejection {
    RateLimited,
    TooLarge,
    Invalid(anyhow::Error),
    Unauthorized,
    VtsUnavailable,
    CollectorUnavailable,
}

impl Rejection {
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::VtsUnavailable | Self::CollectorUnavailable => StatusCode::BAD_GATEWAY,
        }
    }

    pub(crate) fn code(&self) -> String {
        match self {
            Self::RateLimited => "rate_limited".to_string(),
            Self::TooLarge => "too_large".to_string(),
            Self::Invalid(error) => format!("invalid_report: {error:#}"),
            Self::Unauthorized => "unauthorized".to_string(),
            Self::VtsUnavailable => "vts_unavailable".to_string(),
            Self::CollectorUnavailable => "collector_unavailable".to_string(),
        }
    }
}

impl ClientTelemetry {
    /// `None` when no OTLP endpoint is set, so there is nowhere to forward to.
    pub fn new(config: ClientTelemetryConfig) -> anyhow::Result<Option<Self>> {
        if config.otlp_logs_endpoint.is_none() && config.otlp_metrics_endpoint.is_none() {
            return Ok(None);
        }
        Ok(Some(Self {
            http: reqwest::Client::new(),
            verifier: config.vts_url.map(AppVerifier::new).transpose()?,
            otlp_logs_endpoint: config.otlp_logs_endpoint,
            otlp_metrics_endpoint: config.otlp_metrics_endpoint,
            rate_limiter: RateLimiter::new(),
        }))
    }

    pub(crate) async fn receive(
        &self,
        peer: IpAddr,
        headers: &HeaderMap,
        body: Incoming,
    ) -> Result<(), Rejection> {
        if !self.rate_limiter.allow(client_address(headers, peer)) {
            return Err(Rejection::RateLimited);
        }
        let body = match Limited::new(body, MAX_BODY_BYTES).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) if error.downcast_ref::<LengthLimitError>().is_some() => {
                return Err(Rejection::TooLarge);
            }
            Err(error) => return Err(Rejection::Invalid(anyhow::anyhow!(error))),
        };
        let report = ClientReport::parse(&body).map_err(Rejection::Invalid)?;
        let app_id = self.app_id(report.token.as_deref()).await?;
        self.export(&report, &app_id).await
    }

    async fn app_id(&self, token: Option<&str>) -> Result<String, Rejection> {
        let (Some(verifier), Some(token)) = (&self.verifier, token) else {
            return Ok(ANONYMOUS_APP_ID.to_string());
        };
        verifier.app_id(token).await.map_err(|error| match error {
            VerifyError::Unauthorized(_) => {
                tracing::debug!(%error, "client telemetry rejected");
                Rejection::Unauthorized
            }
            VerifyError::Unavailable(_) => {
                tracing::warn!(%error, "client telemetry was not verified");
                Rejection::VtsUnavailable
            }
        })
    }

    async fn export(&self, report: &ClientReport, app_id: &str) -> Result<(), Rejection> {
        let logs = self.forward(
            self.otlp_logs_endpoint.as_deref(),
            otlp_export::logs_request(report, app_id),
        );
        let metrics = self.forward(
            self.otlp_metrics_endpoint.as_deref(),
            otlp_export::metrics_request(report, app_id),
        );
        let (logs, metrics) = tokio::join!(logs, metrics);
        logs.and(metrics).map_err(|error| {
            tracing::warn!(%error, app_id, "client telemetry was not exported");
            Rejection::CollectorUnavailable
        })
    }

    async fn forward(
        &self,
        endpoint: Option<&str>,
        request: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        match (endpoint, request) {
            (Some(endpoint), Some(request)) => {
                otlp::post(&self.http, endpoint, request.to_string()).await
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use hyper::StatusCode;
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, sync::mpsc};

    use super::{ClientTelemetry, ClientTelemetryConfig, MAX_BODY_BYTES};
    use crate::{
        api_server::{ApiServer, ApiState},
        clickhouse::ClickHouse,
        config::ClickHouseConfig,
        latest_snapshots::LatestSnapshots,
        test_support::{fake_http_server, fake_http_server_answering},
    };

    const VERIFIED: &str = r#"{"appId":"app-1","isRelay":false,"claims":{"exp":4102444800}}"#;

    struct Observability {
        url: String,
        _api: ApiServer,
    }

    struct Collector {
        url: String,
        request_receiver: mpsc::UnboundedReceiver<(String, String)>,
    }

    async fn collector() -> Collector {
        let (url, request_receiver) = fake_http_server().await;
        Collector {
            url,
            request_receiver,
        }
    }

    async fn observability(collector: &Collector, vts_url: Option<String>) -> Observability {
        let client_telemetry = ClientTelemetry::new(ClientTelemetryConfig {
            vts_url,
            otlp_logs_endpoint: Some(format!("{}/v1/logs", collector.url)),
            otlp_metrics_endpoint: Some(format!("{}/v1/metrics", collector.url)),
        })
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/api/client-telemetry",
            listener.local_addr().unwrap()
        );
        let clickhouse = ClickHouse::new(ClickHouseConfig {
            url: "http://127.0.0.1:1".to_string(),
            database: "observability".to_string(),
            user: None,
            password: None,
        });
        let api = ApiServer::run(
            listener,
            ApiState {
                latest: LatestSnapshots::default(),
                clickhouse,
                client_telemetry,
            },
        );
        Observability { url, _api: api }
    }

    fn report(token: Option<&str>) -> Value {
        json!({
            "token": token,
            "clientId": "c1",
            "page": "live-viewer",
            "logs": [{ "timeMs": 1, "level": "warn", "target": "moqt", "message": "m" }],
            "stats": [{ "timeMs": 1, "values": { "bufferMs": 100 } }],
        })
    }

    async fn post(observability: &Observability, body: String) -> StatusCode {
        reqwest::Client::new()
            .post(&observability.url)
            .header("content-type", "text/plain")
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn an_anonymous_report_is_forwarded_as_anon_logs_and_metrics() {
        // Arrange
        let mut collector = collector().await;
        let observability = observability(&collector, None).await;

        // Act
        let status = post(&observability, report(None).to_string()).await;

        // Assert
        assert_eq!(status, StatusCode::ACCEPTED);
        let mut forwarded = [
            collector.request_receiver.recv().await.unwrap(),
            collector.request_receiver.recv().await.unwrap(),
        ];
        forwarded.sort();
        assert_eq!(forwarded[0].0, "/v1/logs");
        assert_eq!(forwarded[1].0, "/v1/metrics");
        assert!(forwarded.iter().all(|(_, body)| body.contains("\"anon\"")));
    }

    #[tokio::test]
    async fn a_verified_token_names_the_app_of_the_report() {
        // Arrange
        let mut collector = collector().await;
        let (vts_url, _vts_requests) = fake_http_server_answering(StatusCode::OK, VERIFIED).await;
        let observability = observability(&collector, Some(format!("{vts_url}/verify"))).await;

        // Act
        let status = post(&observability, report(Some("jwt")).to_string()).await;

        // Assert
        assert_eq!(status, StatusCode::ACCEPTED);
        let (_, body) = collector.request_receiver.recv().await.unwrap();
        assert!(body.contains("\"app-1\""), "{body}");
    }

    #[tokio::test]
    async fn a_rejected_token_forwards_nothing() {
        // Arrange
        let mut collector = collector().await;
        let (vts_url, _vts_requests) =
            fake_http_server_answering(StatusCode::UNAUTHORIZED, r#"{"error":"bad"}"#).await;
        let observability = observability(&collector, Some(format!("{vts_url}/verify"))).await;

        // Act
        let status = post(&observability, report(Some("jwt")).to_string()).await;

        // Assert
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(collector.request_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_body_over_the_limit_is_too_large() {
        // Arrange
        let collector = collector().await;
        let observability = observability(&collector, None).await;

        // Act
        let status = post(&observability, "x".repeat(MAX_BODY_BYTES + 1)).await;

        // Assert
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_client_past_its_burst_is_rate_limited() {
        // Arrange
        let collector = collector().await;
        let observability = observability(&collector, None).await;
        let body = json!({ "clientId": "c1", "page": "p" }).to_string();
        let mut statuses = Vec::new();

        // Act
        for _ in 0..11 {
            statuses.push(post(&observability, body.clone()).await);
        }

        // Assert
        assert!(
            statuses[..10]
                .iter()
                .all(|status| *status == StatusCode::ACCEPTED)
        );
        assert_eq!(statuses[10], StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn without_an_otlp_endpoint_there_is_no_receiver() {
        // Act
        let client_telemetry = ClientTelemetry::new(ClientTelemetryConfig {
            vts_url: None,
            otlp_logs_endpoint: None,
            otlp_metrics_endpoint: None,
        })
        .unwrap();

        // Assert
        assert!(client_telemetry.is_none());
    }
}

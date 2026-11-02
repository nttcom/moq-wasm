use std::future;

use observability::{
    api_server::{ApiServer, ApiState},
    clickhouse::ClickHouse,
    client_telemetry::{ClientTelemetry, ClientTelemetryConfig},
    config::ObservabilityConfig,
    schema,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _telemetry = telemetry::init(telemetry::TelemetryConfig {
        service_name: "observability",
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["observability=info"],
        resource_attributes: Vec::new(),
        log_output: telemetry::LogOutput::Stdout,
    })?;
    let config = ObservabilityConfig::from_env()?;
    let clickhouse = ClickHouse::new(config.clickhouse.clone());
    schema::apply(&clickhouse).await?;

    let client_telemetry = ClientTelemetry::new(ClientTelemetryConfig {
        vts_url: config.vts_url.clone(),
        otlp_logs_endpoint: config.otlp_logs_endpoint.clone(),
        otlp_metrics_endpoint: config.otlp_metrics_endpoint.clone(),
    })?;
    let listener = TcpListener::bind(("0.0.0.0", config.http_port)).await?;
    let _api = ApiServer::run(
        listener,
        ApiState {
            clickhouse,
            client_telemetry,
        },
    );
    tracing::info!(
        http_port = config.http_port,
        otlp_metrics = config.otlp_metrics_endpoint.is_some(),
        otlp_logs = config.otlp_logs_endpoint.is_some(),
        client_token_verification = config.vts_url.is_some(),
        "observability started"
    );

    future::pending().await
}

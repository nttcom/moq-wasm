use anyhow::Context;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClickHouseConfig {
    pub url: String,
    pub database: String,
    pub user: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub clickhouse: ClickHouseConfig,
    pub http_port: u16,
    /// The full OTLP/HTTP metrics URL (`.../v1/metrics`) for player stats; `None` drops them.
    pub otlp_metrics_endpoint: Option<String>,
    /// The full OTLP/HTTP logs URL (`.../v1/logs`) for browser logs; `None` drops them.
    pub otlp_logs_endpoint: Option<String>,
    /// The full OTLP/HTTP traces URL (`.../v1/traces`) for client request spans; `None` drops them.
    pub otlp_traces_endpoint: Option<String>,
    /// The VTS `.../verify` URL that reads a browser report's app from its token.
    pub vts_url: Option<String>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

impl ObservabilityConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let http_port = env("OBSERVABILITY_HTTP_PORT")
            .map(|port| port.parse::<u16>())
            .transpose()
            .context("OBSERVABILITY_HTTP_PORT must be a port number")?
            .unwrap_or(8095);
        Ok(Self {
            clickhouse: ClickHouseConfig {
                url: env("CLICKHOUSE_URL").unwrap_or_else(|| "http://127.0.0.1:8123".to_string()),
                database: env("CLICKHOUSE_DATABASE").unwrap_or_else(|| "observability".to_string()),
                user: env("CLICKHOUSE_USER"),
                password: env("CLICKHOUSE_PASSWORD"),
            },
            http_port,
            otlp_metrics_endpoint: env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"),
            otlp_logs_endpoint: env("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"),
            otlp_traces_endpoint: env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"),
            vts_url: env("AUTH_VTS_URL"),
        })
    }
}

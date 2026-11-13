pub mod api_server;
pub mod app_verifier;
mod browser_json;
pub mod clickhouse;
pub mod client_telemetry;
pub mod config;
pub mod namespaces;
mod otlp;
pub mod schema;
pub mod series;
pub mod snapshot_history;
#[cfg(test)]
mod test_support;
pub mod usage;
mod viewer;

/// The app of a request or report without a token, as the relay names an
/// anonymous session's app.
const ANONYMOUS_APP_ID: &str = "anon";

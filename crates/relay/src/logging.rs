use telemetry::{LogOutput, TelemetryConfig, TelemetryGuard};

use crate::config::{non_empty_env, relay_id};

pub(crate) fn relay_hostname() -> String {
    non_empty_env("RELAY_HOSTNAME")
        .or_else(|| non_empty_env("HOSTNAME"))
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn init_logging(service_name: &str) -> anyhow::Result<TelemetryGuard> {
    let relay_hostname = relay_hostname();
    telemetry::init(TelemetryConfig {
        service_name,
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["relay=info", "moqt=info"],
        resource_attributes: vec![
            ("host.name", relay_hostname.clone()),
            ("relay.hostname", relay_hostname),
            ("relay.id", relay_id()),
        ],
        log_output: LogOutput::Stdout,
    })
}

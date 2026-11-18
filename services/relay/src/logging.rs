use std::sync::OnceLock;

use opentelemetry::trace::{Status, TraceContextExt as _};
use telemetry_otlp::{LogOutput, TelemetryConfig, TelemetryGuard, non_empty_env};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::config::relay_id;

/// For a span that roots its own trace but starts from another span's work.
pub(crate) fn link_span(span: &Span, linked: &Span) {
    span.add_link(linked.context().span().span_context().clone());
}

/// The trace exporter drops log events, so an ERROR log alone leaves its span OK.
pub(crate) fn mark_current_span_failed(description: String) {
    Span::current().set_status(Status::error(description));
}

pub(crate) fn relay_hostname() -> &'static str {
    static RELAY_HOSTNAME: OnceLock<String> = OnceLock::new();
    RELAY_HOSTNAME.get_or_init(|| {
        non_empty_env("RELAY_HOSTNAME")
            .or_else(|| non_empty_env("HOSTNAME"))
            .unwrap_or_else(|| "unknown".to_string())
    })
}

pub fn init_logging(service_name: &str) -> anyhow::Result<TelemetryGuard> {
    moqt::set_trace_context_provider(telemetry_otlp::current_traceparent);
    let relay_hostname = relay_hostname();
    telemetry_otlp::init(TelemetryConfig {
        service_name,
        service_version: env!("CARGO_PKG_VERSION"),
        default_directives: &["relay=info", "moqt=info"],
        resource_attributes: vec![
            ("host.name", relay_hostname.to_string()),
            ("relay.hostname", relay_hostname.to_string()),
            ("relay.id", relay_id()),
        ],
        log_output: LogOutput::Stdout,
    })
}

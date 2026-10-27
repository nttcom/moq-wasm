use anyhow::{Context, Result};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLoggerProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt};

mod http_trace_context;

pub use http_trace_context::{continue_trace_from, inject_trace_context};

#[derive(Clone, Copy)]
enum OtlpProtocol {
    Grpc,
    HttpProtobuf,
}

/// Where the human-readable log goes; OTLP logs are unaffected.
pub enum LogOutput {
    Stdout,
    Stderr,
}

pub struct TelemetryConfig<'a> {
    /// `service.name`, unless `OTEL_SERVICE_NAME` overrides it. Also prefixes
    /// the filter variables, each falling back to `RUST_LOG`. For `relay`:
    /// - `RELAY_STDOUT_FILTER`: the human-readable log, on either `log_output`
    /// - `RELAY_OTEL_FILTER`: OTLP spans
    /// - `RELAY_LOG_FILTER`: OTLP logs
    pub service_name: &'a str,
    pub service_version: &'a str,
    /// Added to every filter that has no directive for the same target.
    pub default_directives: &'a [&'a str],
    pub resource_attributes: Vec<(&'static str, String)>,
    pub log_output: LogOutput,
}

/// Hold it until the process exits: dropping it flushes and shuts down the
/// OTLP exporters.
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn env_flag_is_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Unlike the OpenTelemetry default, no `OTEL_EXPORTER_OTLP_ENDPOINT` means
/// no export rather than export to `localhost:4317`.
fn otel_is_enabled() -> bool {
    non_empty_env("OTEL_EXPORTER_OTLP_ENDPOINT").is_some()
        && !env_flag_is_enabled("OTEL_SDK_DISABLED")
}

fn otel_protocol() -> Result<OtlpProtocol> {
    let protocol =
        non_empty_env("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|| "grpc".to_string());

    match protocol.to_ascii_lowercase().as_str() {
        "grpc" => Ok(OtlpProtocol::Grpc),
        "http/protobuf" | "http/proto" | "http" => Ok(OtlpProtocol::HttpProtobuf),
        _ => anyhow::bail!(
            "unsupported OTEL_EXPORTER_OTLP_PROTOCOL: {protocol}. supported values are grpc and http/protobuf"
        ),
    }
}

fn otel_protocol_name(protocol: OtlpProtocol) -> &'static str {
    match protocol {
        OtlpProtocol::Grpc => "grpc",
        OtlpProtocol::HttpProtobuf => "http/protobuf",
    }
}

fn otel_service_name(default_service_name: &str) -> String {
    non_empty_env("OTEL_SERVICE_NAME").unwrap_or_else(|| default_service_name.to_string())
}

fn otel_resource(config: &TelemetryConfig, service_name: &str) -> Resource {
    Resource::builder_empty()
        .with_attributes(
            [
                ("service.name", service_name.to_string()),
                ("service.version", config.service_version.to_string()),
            ]
            .into_iter()
            .chain(config.resource_attributes.iter().cloned())
            .map(|(key, value)| opentelemetry::KeyValue::new(key, value)),
        )
        .build()
}

fn append_directive_if_missing(filter: &mut String, directive: &str) {
    let target = directive.split('=').next().unwrap_or(directive).trim();
    let exists = filter.split(',').map(str::trim).any(|part| {
        part == target
            || part.starts_with(&format!("{target}="))
            || part.starts_with(&format!("{target}["))
    });

    if !exists {
        if !filter.is_empty() {
            filter.push(',');
        }
        filter.push_str(directive);
    }
}

fn filter_env_name(service_name: &str, output: &str) -> String {
    format!(
        "{}_{output}_FILTER",
        service_name.to_ascii_uppercase().replace('-', "_")
    )
}

fn tracing_filter(config: &TelemetryConfig, output: &str) -> EnvFilter {
    let mut filter = non_empty_env(&filter_env_name(config.service_name, output))
        .or_else(|| non_empty_env("RUST_LOG"))
        .unwrap_or_default();

    for directive in config.default_directives.iter().chain(&["telemetry=info"]) {
        append_directive_if_missing(&mut filter, directive);
    }
    EnvFilter::new(filter)
}

fn build_tracer_provider(resource: Resource, protocol: OtlpProtocol) -> Result<SdkTracerProvider> {
    let exporter = match protocol {
        OtlpProtocol::Grpc => opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build(),
        OtlpProtocol::HttpProtobuf => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .build(),
    }
    .context("failed to build OTLP span exporter")?;

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build())
}

fn build_logger_provider(resource: Resource, protocol: OtlpProtocol) -> Result<SdkLoggerProvider> {
    let exporter = match protocol {
        OtlpProtocol::Grpc => opentelemetry_otlp::LogExporter::builder()
            .with_tonic()
            .build(),
        OtlpProtocol::HttpProtobuf => opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .build(),
    }
    .context("failed to build OTLP log exporter")?;

    Ok(SdkLoggerProvider::builder()
        .with_log_processor(BatchLogProcessor::builder(exporter).build())
        .with_resource(resource)
        .build())
}

/// Installs the global `tracing` subscriber; call it before the first log.
/// Fails when a subscriber is already installed, `OTEL_EXPORTER_OTLP_PROTOCOL`
/// is unsupported, or an OTLP exporter cannot be built.
pub fn init(config: TelemetryConfig) -> Result<TelemetryGuard> {
    let writer = match config.log_output {
        LogOutput::Stdout => BoxMakeWriter::new(std::io::stdout),
        LogOutput::Stderr => BoxMakeWriter::new(std::io::stderr),
    };
    let stdout_layer = fmt::layer()
        .with_writer(writer)
        .with_ansi(true)
        .with_line_number(true)
        .with_filter(tracing_filter(&config, "STDOUT"));

    let service_name = otel_service_name(config.service_name);
    let protocol = otel_protocol()?;
    let (tracer_provider, logger_provider) = if otel_is_enabled() {
        let resource = otel_resource(&config, &service_name);
        (
            Some(build_tracer_provider(resource.clone(), protocol)?),
            Some(build_logger_provider(resource, protocol)?),
        )
    } else {
        (None, None)
    };

    let otel_trace_layer = tracer_provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer(service_name.clone()))
            .with_location(false)
            .with_threads(false)
            .with_tracked_inactivity(false)
            .with_target(false)
            .with_filter(filter_fn(|metadata| metadata.is_span()))
            .with_filter(tracing_filter(&config, "OTEL"))
    });

    let otel_log_layer = logger_provider.as_ref().map(|provider| {
        OpenTelemetryTracingBridge::new(provider).with_filter(tracing_filter(&config, "LOG"))
    });

    if let Some(provider) = tracer_provider.as_ref() {
        global::set_text_map_propagator(TraceContextPropagator::new());
        global::set_tracer_provider(provider.clone());
    }

    Registry::default()
        .with(stdout_layer)
        .with(otel_trace_layer)
        .with(otel_log_layer)
        .try_init()
        .context("failed to initialize tracing subscriber")?;

    tracing::info!(
        service_name = %service_name,
        resource_attributes = ?config.resource_attributes,
        otel_enabled = tracer_provider.is_some(),
        otlp_protocol = %otel_protocol_name(protocol),
        "logging initialized"
    );

    Ok(TelemetryGuard {
        tracer_provider,
        logger_provider,
    })
}

#[cfg(test)]
mod tests {
    use super::{append_directive_if_missing, filter_env_name};

    #[test]
    fn a_default_directive_is_added_only_for_a_target_the_filter_lacks() {
        // Arrange
        let mut filter = "relay=debug,quinn=warn".to_string();

        // Act
        append_directive_if_missing(&mut filter, "relay=info");
        append_directive_if_missing(&mut filter, "moqt=info");

        // Assert
        assert_eq!(filter, "relay=debug,quinn=warn,moqt=info");
    }

    #[test]
    fn the_filter_variable_is_named_after_the_service() {
        // Act / Assert
        assert_eq!(
            filter_env_name("live-ingest", "STDOUT"),
            "LIVE_INGEST_STDOUT_FILTER"
        );
    }
}

use anyhow::{Context, Result};
use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt as _, TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLoggerProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt};

use crate::config::{non_empty_env, relay_id};

#[derive(Clone, Copy)]
enum OtlpProtocol {
    Grpc,
    HttpProtobuf,
}

pub struct LoggingGuards {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Drop for LoggingGuards {
    fn drop(&mut self) {
        if let Some(provider) = self.logger_provider.take() {
            let _ = provider.shutdown();
        }
        if let Some(provider) = self.tracer_provider.take() {
            let _ = provider.shutdown();
        }
    }
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

fn otel_is_enabled() -> bool {
    !env_flag_is_enabled("OTEL_SDK_DISABLED")
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

/// Links `span` to `linked`, for a span that roots its own trace yet starts
/// from another span's work.
pub(crate) fn link_span(span: &Span, linked: &Span) {
    span.add_link(linked.context().span().span_context().clone());
}

pub(crate) fn relay_hostname() -> String {
    non_empty_env("RELAY_HOSTNAME")
        .or_else(|| non_empty_env("HOSTNAME"))
        .unwrap_or_else(|| "unknown".to_string())
}

fn otel_resource(service_name: &str) -> Resource {
    let relay_hostname = relay_hostname();
    Resource::builder_empty()
        .with_attributes([
            opentelemetry::KeyValue::new("service.name", service_name.to_string()),
            opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            opentelemetry::KeyValue::new("host.name", relay_hostname.clone()),
            opentelemetry::KeyValue::new("relay.hostname", relay_hostname),
            opentelemetry::KeyValue::new("relay.id", relay_id()),
        ])
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

const DEFAULT_DIRECTIVES: &[&str] = &["relay=info", "moqt=info"];

fn tracing_filter(filter_env: &str) -> EnvFilter {
    let mut filter = non_empty_env(filter_env)
        .or_else(|| non_empty_env("RUST_LOG"))
        .unwrap_or_default();

    for directive in DEFAULT_DIRECTIVES {
        append_directive_if_missing(&mut filter, directive);
    }
    EnvFilter::new(filter)
}

fn build_tracer_provider(service_name: &str, protocol: OtlpProtocol) -> Result<SdkTracerProvider> {
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
        .with_resource(otel_resource(service_name))
        .build())
}

fn build_logger_provider(service_name: &str, protocol: OtlpProtocol) -> Result<SdkLoggerProvider> {
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
        .with_resource(otel_resource(service_name))
        .build())
}

pub fn init_logging(default_service_name: &str) -> Result<LoggingGuards> {
    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_line_number(true)
        .with_filter(tracing_filter("RELAY_STDOUT_FILTER"));

    let service_name = otel_service_name(default_service_name);
    let protocol = otel_protocol()?;
    let (tracer_provider, logger_provider) = if otel_is_enabled() {
        (
            Some(build_tracer_provider(&service_name, protocol)?),
            Some(build_logger_provider(&service_name, protocol)?),
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
            .with_filter(tracing_filter("RELAY_OTEL_FILTER"))
    });

    let otel_log_layer = logger_provider.as_ref().map(|provider| {
        OpenTelemetryTracingBridge::new(provider).with_filter(tracing_filter("RELAY_LOG_FILTER"))
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
        relay_hostname = %relay_hostname(),
        relay_id = %relay_id(),
        otel_enabled = tracer_provider.is_some(),
        otlp_protocol = %otel_protocol_name(protocol),
        "logging initialized"
    );

    Ok(LoggingGuards {
        tracer_provider,
        logger_provider,
    })
}

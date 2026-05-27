use anyhow::{Context, Result};
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{BatchLogProcessor, SdkLoggerProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt};

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
        std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "grpc".to_string());

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

fn append_http_signal_path(endpoint: String, path: &str) -> String {
    if endpoint.ends_with('/') && path.starts_with('/') {
        format!("{}{}", endpoint, &path[1..])
    } else {
        format!("{endpoint}{path}")
    }
}

fn otel_endpoint(protocol: OtlpProtocol) -> String {
    if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
        return endpoint;
    }

    std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .map(|endpoint| match protocol {
            OtlpProtocol::Grpc => endpoint,
            OtlpProtocol::HttpProtobuf => append_http_signal_path(endpoint, "/v1/traces"),
        })
        .unwrap_or_else(|_| match protocol {
            OtlpProtocol::Grpc => "http://localhost:4317".to_string(),
            OtlpProtocol::HttpProtobuf => "http://localhost:4318/v1/traces".to_string(),
        })
}

fn otel_logs_endpoint(protocol: OtlpProtocol) -> String {
    if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT") {
        return endpoint;
    }

    std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .map(|endpoint| match protocol {
            OtlpProtocol::Grpc => endpoint,
            OtlpProtocol::HttpProtobuf => append_http_signal_path(endpoint, "/v1/logs"),
        })
        .unwrap_or_else(|_| match protocol {
            OtlpProtocol::Grpc => "http://localhost:4317".to_string(),
            OtlpProtocol::HttpProtobuf => "http://localhost:4318/v1/logs".to_string(),
        })
}

fn otel_service_name(default_service_name: &str) -> String {
    std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| default_service_name.to_string())
}

fn relay_hostname() -> String {
    std::env::var("RELAY_HOSTNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn relay_id() -> String {
    std::env::var("RELAY_ID").unwrap_or_else(|_| "relay-local".to_string())
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

fn tracing_filter(filter_env: &str, default_directives: &[&str]) -> EnvFilter {
    let mut filter = std::env::var(filter_env)
        .or_else(|_| std::env::var("RUST_LOG"))
        .unwrap_or_default();

    for directive in default_directives {
        append_directive_if_missing(&mut filter, directive);
    }

    if filter.is_empty() {
        EnvFilter::new(default_directives.join(","))
    } else {
        EnvFilter::new(filter)
    }
}

fn build_tracer_provider(
    service_name: &str,
    protocol: OtlpProtocol,
) -> Result<Option<SdkTracerProvider>> {
    if !otel_is_enabled() {
        return Ok(None);
    }

    let exporter = match protocol {
        OtlpProtocol::Grpc => opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(otel_endpoint(protocol))
            .build(),
        OtlpProtocol::HttpProtobuf => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .build(),
    }
    .context("failed to build OTLP span exporter")?;

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(otel_resource(service_name))
        .build();

    Ok(Some(provider))
}

fn build_logger_provider(
    service_name: &str,
    protocol: OtlpProtocol,
) -> Result<Option<SdkLoggerProvider>> {
    if !otel_is_enabled() {
        return Ok(None);
    }

    let exporter = match protocol {
        OtlpProtocol::Grpc => opentelemetry_otlp::LogExporter::builder()
            .with_tonic()
            .with_endpoint(otel_logs_endpoint(protocol))
            .build(),
        OtlpProtocol::HttpProtobuf => opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .build(),
    }
    .context("failed to build OTLP log exporter")?;

    let provider = SdkLoggerProvider::builder()
        .with_log_processor(BatchLogProcessor::builder(exporter).build())
        .with_resource(otel_resource(service_name))
        .build();

    Ok(Some(provider))
}

pub fn init_logging(default_service_name: &str) -> Result<LoggingGuards> {
    let stdout_layer = fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_line_number(true)
        .with_filter(tracing_filter(
            "RELAY_STDOUT_FILTER",
            &["relay=info", "moqt=info"],
        ));

    let service_name = otel_service_name(default_service_name);
    let protocol = otel_protocol()?;
    let tracer_provider = build_tracer_provider(&service_name, protocol)?;
    let logger_provider = build_logger_provider(&service_name, protocol)?;

    let otel_trace_layer = tracer_provider.as_ref().map(|provider| {
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer(service_name.clone()))
            .with_location(true)
            .with_threads(true)
            .with_tracked_inactivity(false)
            .with_target(false)
            .with_filter(filter_fn(|metadata| metadata.is_span()))
            .with_filter(tracing_filter(
                "RELAY_OTEL_FILTER",
                &["relay=info", "moqt=info"],
            ))
    });

    let otel_log_layer = logger_provider.as_ref().map(|provider| {
        OpenTelemetryTracingBridge::new(provider).with_filter(tracing_filter(
            "RELAY_LOG_FILTER",
            &["relay=info", "moqt=info"],
        ))
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
        otlp_endpoint = %otel_endpoint(protocol),
        otlp_logs_endpoint = %otel_logs_endpoint(protocol),
        "logging initialized"
    );

    Ok(LoggingGuards {
        tracer_provider,
        logger_provider,
    })
}

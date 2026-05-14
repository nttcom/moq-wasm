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

fn otel_endpoint() -> String {
    std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT"))
        .unwrap_or_else(|_| "http://localhost:4317".to_string())
}

fn otel_logs_endpoint() -> String {
    std::env::var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")
        .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT"))
        .unwrap_or_else(|_| "http://localhost:4317".to_string())
}

fn otel_service_name(default_service_name: &str) -> String {
    std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| default_service_name.to_string())
}

fn otel_resource(service_name: &str) -> Resource {
    Resource::builder_empty()
        .with_attributes([
            opentelemetry::KeyValue::new("service.name", service_name.to_string()),
            opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
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

fn build_tracer_provider(service_name: &str) -> Result<Option<SdkTracerProvider>> {
    if !otel_is_enabled() {
        return Ok(None);
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(otel_endpoint())
        .build()
        .context("failed to build OTLP span exporter")?;

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(otel_resource(service_name))
        .build();

    Ok(Some(provider))
}

fn build_logger_provider(service_name: &str) -> Result<Option<SdkLoggerProvider>> {
    if !otel_is_enabled() {
        return Ok(None);
    }

    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(otel_logs_endpoint())
        .build()
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
    let tracer_provider = build_tracer_provider(&service_name)?;
    let logger_provider = build_logger_provider(&service_name)?;

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
        otel_enabled = tracer_provider.is_some(),
        otlp_endpoint = %otel_endpoint(),
        otlp_logs_endpoint = %otel_logs_endpoint(),
        "logging initialized"
    );

    Ok(LoggingGuards {
        tracer_provider,
        logger_provider,
    })
}

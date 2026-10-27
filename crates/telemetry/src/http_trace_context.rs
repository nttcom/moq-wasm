use http::{HeaderMap, HeaderName, HeaderValue};
use opentelemetry::global;
use opentelemetry::propagation::{Extractor, Injector};
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

struct HeaderInjector<'a>(&'a mut HeaderMap);

impl Injector for HeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(key), HeaderValue::try_from(value)) {
            self.0.insert(name, value);
        }
    }
}

struct HeaderExtractor<'a>(&'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(HeaderName::as_str).collect()
    }
}

/// Writes `span`'s trace context into `headers` as W3C `traceparent`.
/// Writes nothing while OTLP is off.
pub fn inject_trace_context(span: &Span, headers: &mut HeaderMap) {
    let context = span.context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(headers));
    });
}

/// Makes `span` a child of the remote span whose context `headers` carry.
/// Call it before `span` is first entered; a request without a valid
/// `traceparent` leaves `span` a trace root.
pub fn continue_trace_from(span: &Span, headers: &HeaderMap) {
    let context =
        global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)));
    if let Err(error) = span.set_parent(context) {
        tracing::debug!(%error, "failed to continue the caller's trace");
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderMap;
    use opentelemetry::global;
    use opentelemetry::trace::{TraceContextExt as _, TraceId, TracerProvider as _};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing::Span;
    use tracing::subscriber::DefaultGuard;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{continue_trace_from, inject_trace_context};

    fn install_otel_subscriber() -> DefaultGuard {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::set_default(subscriber)
    }

    fn trace_id(span: &Span) -> TraceId {
        span.context().span().span_context().trace_id()
    }

    #[test]
    fn a_span_continued_from_injected_headers_joins_the_caller_s_trace() {
        // Arrange
        let _default = install_otel_subscriber();
        let caller = tracing::info_span!("caller");
        let callee = tracing::info_span!("callee");
        let mut headers = HeaderMap::new();

        // Act
        inject_trace_context(&caller, &mut headers);
        continue_trace_from(&callee, &headers);

        // Assert
        assert!(headers.contains_key("traceparent"));
        assert_eq!(trace_id(&callee), trace_id(&caller));
    }

    #[test]
    fn a_request_without_a_traceparent_leaves_the_span_a_root() {
        // Arrange
        let _default = install_otel_subscriber();
        let other = tracing::info_span!("other");
        let callee = tracing::info_span!("callee");

        // Act
        continue_trace_from(&callee, &HeaderMap::new());

        // Assert
        assert!(callee.context().span().span_context().is_valid());
        assert_ne!(trace_id(&callee), trace_id(&other));
    }
}

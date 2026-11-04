use std::collections::HashMap;

use http::{HeaderMap, HeaderName, HeaderValue};
use opentelemetry::global;
use opentelemetry::propagation::Injector;
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

const TRACEPARENT: &str = "traceparent";

/// A copy of `traceparent` that Cloud Run's front end leaves alone; the front
/// end replaces the parent in `traceparent` with its own span, which only
/// Cloud Trace receives.
const ORIGINAL_TRACEPARENT: &str = "x-original-traceparent";

/// Writes `span`'s trace context into `headers` as W3C `traceparent`, and
/// again as `x-original-traceparent`. Writes nothing while OTLP is off.
pub fn inject_trace_context(span: &Span, headers: &mut HeaderMap) {
    let context = span.context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(headers));
    });
    if let Some(traceparent) = headers.get(TRACEPARENT).cloned() {
        headers.insert(ORIGINAL_TRACEPARENT, traceparent);
    }
}

/// Makes `span` a child of the remote span whose context `headers` carry,
/// read from `x-original-traceparent` when present, else from `traceparent`.
/// Call it before `span` is first entered; a request without a valid one
/// leaves `span` a trace root.
pub fn continue_trace_from(span: &Span, headers: &HeaderMap) {
    let traceparent = headers
        .get(ORIGINAL_TRACEPARENT)
        .or_else(|| headers.get(TRACEPARENT))
        .and_then(|value| value.to_str().ok());
    if let Some(traceparent) = traceparent {
        continue_trace_from_traceparent(span, traceparent);
    }
}

/// The current span's context as a W3C `traceparent`; `None` while OTLP is
/// off or outside any span.
pub fn current_traceparent() -> Option<String> {
    let context = Span::current().context();
    let mut carrier = HashMap::new();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut carrier);
    });
    carrier.remove(TRACEPARENT)
}

/// Makes `span` a child of the remote span `traceparent` names. Call it
/// before `span` is first entered; an invalid `traceparent` leaves `span` a
/// trace root.
pub fn continue_trace_from_traceparent(span: &Span, traceparent: &str) {
    let carrier = HashMap::from([(TRACEPARENT.to_string(), traceparent.to_string())]);
    let context = global::get_text_map_propagator(|propagator| propagator.extract(&carrier));
    if let Err(error) = span.set_parent(context) {
        tracing::debug!(%error, "failed to continue the caller's trace");
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderMap;
    use opentelemetry::global;
    use opentelemetry::trace::{SpanId, TraceContextExt as _, TraceId, TracerProvider as _};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing::Span;
    use tracing::subscriber::DefaultGuard;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{
        continue_trace_from, continue_trace_from_traceparent, current_traceparent,
        inject_trace_context,
    };

    fn install_otel_subscriber() -> DefaultGuard {
        install_exporting_otel_subscriber(InMemorySpanExporter::default())
    }

    fn install_exporting_otel_subscriber(exporter: InMemorySpanExporter) -> DefaultGuard {
        global::set_text_map_propagator(TraceContextPropagator::new());
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter)
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::set_default(subscriber)
    }

    fn span_id(span: &Span) -> SpanId {
        span.context().span().span_context().span_id()
    }

    fn exported_parent_of(exporter: &InMemorySpanExporter, name: &str) -> SpanId {
        exporter
            .get_finished_spans()
            .unwrap()
            .into_iter()
            .find(|span| span.name == name)
            .unwrap()
            .parent_span_id
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
    fn a_span_continued_from_headers_whose_traceparent_a_proxy_rewrote_keeps_the_caller_as_parent()
    {
        // Arrange
        let exporter = InMemorySpanExporter::default();
        let _default = install_exporting_otel_subscriber(exporter.clone());
        let caller = tracing::info_span!("caller");
        let proxy = tracing::info_span!("proxy");
        let callee = tracing::info_span!("callee");
        let mut headers = HeaderMap::new();
        inject_trace_context(&caller, &mut headers);
        let mut rewritten = HeaderMap::new();
        inject_trace_context(&proxy, &mut rewritten);
        headers.insert("traceparent", rewritten["traceparent"].clone());

        // Act
        continue_trace_from(&callee, &headers);
        drop(callee);

        // Assert
        assert_eq!(exported_parent_of(&exporter, "callee"), span_id(&caller));
    }

    #[test]
    fn a_span_continued_from_a_traceparent_alone_has_the_caller_as_parent() {
        // Arrange
        let exporter = InMemorySpanExporter::default();
        let _default = install_exporting_otel_subscriber(exporter.clone());
        let caller = tracing::info_span!("caller");
        let callee = tracing::info_span!("callee");
        let mut injected = HeaderMap::new();
        inject_trace_context(&caller, &mut injected);
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", injected["traceparent"].clone());

        // Act
        continue_trace_from(&callee, &headers);
        drop(callee);

        // Assert
        assert_eq!(exported_parent_of(&exporter, "callee"), span_id(&caller));
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

    #[test]
    fn a_span_continued_from_the_current_traceparent_joins_the_caller_s_trace() {
        // Arrange
        let _default = install_otel_subscriber();
        let caller = tracing::info_span!("caller");
        let callee = tracing::info_span!("callee");

        // Act
        let traceparent = caller.in_scope(current_traceparent).unwrap();
        continue_trace_from_traceparent(&callee, &traceparent);

        // Assert
        assert_eq!(trace_id(&callee), trace_id(&caller));
    }

    #[test]
    fn outside_any_span_there_is_no_traceparent() {
        // Arrange
        let _default = install_otel_subscriber();

        // Act / Assert
        assert_eq!(current_traceparent(), None);
    }

    #[test]
    fn an_invalid_traceparent_leaves_the_span_a_root() {
        // Arrange
        let _default = install_otel_subscriber();
        let other = tracing::info_span!("other");
        let callee = tracing::info_span!("callee");

        // Act
        continue_trace_from_traceparent(&callee, "not-a-traceparent");

        // Assert
        assert!(callee.context().span().span_context().is_valid());
        assert_ne!(trace_id(&callee), trace_id(&other));
    }
}

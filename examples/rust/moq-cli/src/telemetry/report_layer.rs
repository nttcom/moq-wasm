use std::{
    fmt::{Debug, Write},
    hash::{BuildHasher, Hasher, RandomState},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value, json};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use super::ReportBuffer;

/// Copies each event and each closed span it sees into a `ReportBuffer`, in
/// the log and span shapes of the observability server's `/api/client-telemetry`.
pub struct ReportLayer(pub ReportBuffer);

struct OpenSpan {
    trace_id: String,
    span_id: String,
    parent_span_id: Option<String>,
    start_ms: u64,
    fields: FieldVisitor,
}

impl<S> Layer<S> for ReportLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        self.0.push_log(json!({
            "timeMs": now_ms(),
            "level": metadata.level().as_str().to_ascii_lowercase(),
            "target": metadata.target(),
            "message": visitor.message,
            "fields": visitor.fields,
        }));
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let parent = span.parent().and_then(|parent| {
            let extensions = parent.extensions();
            let open = extensions.get::<OpenSpan>()?;
            Some((open.trace_id.clone(), open.span_id.clone()))
        });
        let mut fields = FieldVisitor::default();
        attrs.record(&mut fields);
        let (trace_id, parent_span_id) = match parent {
            Some((trace_id, span_id)) => (trace_id, Some(span_id)),
            None => (hex(&[random_u64(), random_u64()]), None),
        };
        span.extensions_mut().insert(OpenSpan {
            trace_id,
            span_id: hex(&[random_u64()]),
            parent_span_id,
            start_ms: now_ms(),
            fields,
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(open) = span.extensions_mut().get_mut::<OpenSpan>()
        {
            values.record(&mut open.fields);
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(open) = span.extensions_mut().remove::<OpenSpan>() else {
            return;
        };
        let mut record = json!({
            "traceId": open.trace_id,
            "spanId": open.span_id,
            "name": span.name(),
            "startTimeMs": open.start_ms,
            "endTimeMs": now_ms(),
            "fields": open.fields.fields,
        });
        if let Some(parent_span_id) = open.parent_span_id {
            record["parentSpanId"] = json!(parent_span_id);
        }
        self.0.push_span(record);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// A random 64-bit value from std's randomly keyed hasher, so the CLI needs no
/// RNG crate: trace and span ids need to be unique, not unpredictable.
fn random_u64() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.finish()
}

fn hex(words: &[u64]) -> String {
    words.iter().fold(String::new(), |mut text, word| {
        let _ = write!(text, "{word:016x}");
        text
    })
}

#[derive(Default)]
struct FieldVisitor {
    message: String,
    fields: Map<String, Value>,
}

impl FieldVisitor {
    fn insert(&mut self, field: &Field, value: Value) {
        self.fields.insert(field.name().to_string(), value);
    }
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.insert(field, json!(value));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, json!(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, json!(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, json!(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.record_str(field, &format!("{value:?}"));
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::{ReportBuffer, ReportLayer};

    #[test]
    fn an_event_becomes_a_log_with_its_message_and_fields() {
        // Arrange
        let buffer = ReportBuffer::default();
        let subscriber = tracing_subscriber::registry().with(ReportLayer(buffer.clone()));

        // Act
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                track_name = "video",
                request_id = 3u64,
                "SUBSCRIBE_ERROR received"
            );
        });

        // Assert
        let (logs, _) = buffer.take();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["level"], "warn");
        assert_eq!(logs[0]["message"], "SUBSCRIBE_ERROR received");
        assert_eq!(logs[0]["fields"]["track_name"], "video");
        assert_eq!(logs[0]["fields"]["request_id"], 3);
    }

    #[test]
    fn a_closed_span_becomes_a_span_with_its_recorded_fields_and_parent() {
        // Arrange
        let buffer = ReportBuffer::default();
        let subscriber = tracing_subscriber::registry().with(ReportLayer(buffer.clone()));

        // Act
        tracing::subscriber::with_default(subscriber, || {
            let _parent = tracing::info_span!("parent").entered();
            let span = tracing::info_span!(
                "moqt.subscriber.subscribe",
                track_name = "video",
                outcome = tracing::field::Empty,
            );
            span.record("outcome", "ok");
        });

        // Assert
        let (_, spans) = buffer.take();
        let [child, parent] = spans.as_slice() else {
            panic!("{spans:?}");
        };
        assert_eq!(child["name"], "moqt.subscriber.subscribe");
        assert_eq!(child["fields"]["track_name"], "video");
        assert_eq!(child["fields"]["outcome"], "ok");
        assert_eq!(child["traceId"], parent["traceId"]);
        assert_eq!(child["parentSpanId"], parent["spanId"]);
        assert_eq!(parent["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(parent["spanId"].as_str().unwrap().len(), 16);
        assert_ne!(child["spanId"], parent["spanId"]);
        assert!(parent.get("parentSpanId").is_none());
    }
}

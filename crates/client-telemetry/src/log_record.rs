use std::fmt;

use serde_json::{Map, Value, json};
use tracing::field::{Field, Visit};
use tracing::{Event, Level};

use crate::span_record::{current_span_ids, hex};

/// `time_ms` is milliseconds since the Unix epoch. Inside a span that
/// `SpanRecordLayer` saw entered, the record also carries that span's
/// `traceId` and `spanId`.
pub fn log_record(event: &Event<'_>, time_ms: u64) -> Value {
    let mut visitor = FieldVisitor::default();
    event.record(&mut visitor);
    let metadata = event.metadata();
    let mut record = json!({
        "timeMs": time_ms,
        "level": level_name(*metadata.level()),
        "target": metadata.target(),
        "message": visitor.message,
        "fields": visitor.fields,
    });
    if let Some((trace_id, span_id)) = current_span_ids() {
        record["traceId"] = json!(hex(&trace_id));
        record["spanId"] = json!(hex(&span_id));
    }
    record
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        Level::DEBUG => "debug",
        Level::TRACE => "trace",
    }
}

/// Collects the fields of an event or a span; an event's `message` is kept apart.
#[derive(Default)]
pub(crate) struct FieldVisitor {
    message: String,
    pub(crate) fields: Map<String, Value>,
}

impl FieldVisitor {
    fn insert(&mut self, field: &Field, value: Value) {
        self.fields.insert(field.name().to_string(), value);
    }
}

impl Visit for FieldVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, json!(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, json!(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, json!(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.insert(field, json!(value));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.insert(field, json!(format!("{value:?}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::Subscriber;
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    use super::*;
    use crate::{SpanRecordLayer, current_traceparent};

    struct CaptureLayer(Arc<Mutex<Vec<Value>>>);

    impl<S: Subscriber> Layer<S> for CaptureLayer {
        fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
            self.0.lock().unwrap().push(log_record(event, 1234));
        }
    }

    fn capture(emit: impl FnOnce()) -> Vec<Value> {
        let records = Arc::new(Mutex::new(Vec::new()));
        let span_layer = SpanRecordLayer {
            now_ms: || 1000,
            random_byte: || 7,
            emit: |_| {},
        };
        let subscriber = tracing_subscriber::registry()
            .with(span_layer)
            .with(CaptureLayer(records.clone()));
        tracing::subscriber::with_default(subscriber, emit);
        std::mem::take(&mut *records.lock().unwrap())
    }

    #[test]
    fn record_carries_level_target_message_and_typed_fields() {
        // Arrange
        let error = "decoder closed";

        // Act
        let records = capture(|| {
            tracing::warn!(
                target: "wasm::media_broadcast",
                %error,
                track_name = "video",
                group_id = 7u64,
                offset = -3i64,
                ratio = 0.5,
                keyframe = true,
                "frame dropped after {} retries",
                2
            );
        });

        // Assert
        assert_eq!(
            records,
            vec![json!({
                "timeMs": 1234,
                "level": "warn",
                "target": "wasm::media_broadcast",
                "message": "frame dropped after 2 retries",
                "fields": {
                    "error": "decoder closed",
                    "track_name": "video",
                    "group_id": 7,
                    "offset": -3,
                    "ratio": 0.5,
                    "keyframe": true,
                },
            })]
        );
    }

    #[test]
    fn record_without_fields_has_an_empty_fields_object() {
        // Act
        let records = capture(|| tracing::error!(target: "moqt", "session closed"));

        // Assert
        assert_eq!(
            records,
            vec![json!({
                "timeMs": 1234,
                "level": "error",
                "target": "moqt",
                "message": "session closed",
                "fields": {},
            })]
        );
    }

    #[test]
    fn a_record_inside_a_span_carries_that_span_s_ids() {
        // Arrange
        let mut traceparent = None;

        // Act
        let records = capture(|| {
            tracing::info_span!("moqt.publisher.publish_namespace").in_scope(|| {
                tracing::info!("PUBLISH_NAMESPACE_OK received");
                traceparent = current_traceparent();
            });
        });

        // Assert
        let [record] = records.as_slice() else {
            panic!("{records:?}");
        };
        let ids = format!(
            "00-{}-{}-01",
            record["traceId"].as_str().unwrap(),
            record["spanId"].as_str().unwrap()
        );
        assert_eq!(traceparent, Some(ids));
    }
}

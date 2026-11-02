use std::{
    fmt::Debug,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value, json};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

use super::LogBuffer;

/// Copies each event it sees into a `LogBuffer`, in the log shape of the
/// observability server's `/api/client-telemetry`.
pub struct ReportLayer(pub LogBuffer);

impl<S: Subscriber> Layer<S> for ReportLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64);
        self.0.push(json!({
            "timeMs": time_ms,
            "level": metadata.level().as_str().to_ascii_lowercase(),
            "target": metadata.target(),
            "message": visitor.message,
            "fields": visitor.fields,
        }));
    }
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

    use super::{LogBuffer, ReportLayer};

    #[test]
    fn an_event_becomes_a_log_with_its_message_and_fields() {
        // Arrange
        let buffer = LogBuffer::default();
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
        let logs = buffer.take();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["level"], "warn");
        assert_eq!(logs[0]["message"], "SUBSCRIBE_ERROR received");
        assert_eq!(logs[0]["fields"]["track_name"], "video");
        assert_eq!(logs[0]["fields"]["request_id"], 3);
    }
}

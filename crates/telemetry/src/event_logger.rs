use std::time::SystemTime;

use opentelemetry::logs::{AnyValue, LogRecord as _, Logger as _, LoggerProvider as _, Severity};
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};

/// Emits OTLP log records that carry data rather than a message: they skip
/// the `tracing` filters and the human-readable log.
#[derive(Clone)]
pub struct EventLogger {
    logger: SdkLogger,
}

impl EventLogger {
    pub(crate) fn new(provider: &SdkLoggerProvider, scope: &'static str) -> Self {
        Self {
            logger: provider.logger(scope),
        }
    }

    /// Queues one INFO record whose `EventName` and body are `event_name`
    /// and whose one attribute is `key` = `value`; the batch exporter sends it
    /// with the process's other logs.
    ///
    /// The data is an attribute, not the body, because the ClickStack
    /// collector parses a body containing a JSON object into one attribute per
    /// field, storing it twice.
    pub fn emit(
        &self,
        event_name: &'static str,
        timestamp: SystemTime,
        key: &'static str,
        value: String,
    ) {
        let mut record = self.logger.create_log_record();
        record.set_event_name(event_name);
        record.set_timestamp(timestamp);
        record.set_severity_number(Severity::Info);
        record.set_severity_text("INFO");
        record.set_body(AnyValue::from(event_name));
        record.add_attribute(key, value);
        self.logger.emit(record);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use opentelemetry::{Key, logs::AnyValue, logs::Severity};
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

    use super::EventLogger;

    #[test]
    fn the_value_travels_as_an_attribute_beside_a_body_naming_the_event() {
        // Arrange
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let logger = EventLogger::new(&provider, "test");
        let timestamp = UNIX_EPOCH + Duration::from_millis(1_793_578_680_991);

        // Act
        logger.emit("test.event", timestamp, "data", r#"{"a":1}"#.to_string());

        // Assert
        let logs = exporter.get_emitted_logs().unwrap();
        let record = &logs[0].record;
        assert_eq!(record.event_name(), Some("test.event"));
        assert_eq!(record.timestamp(), Some(timestamp));
        assert_eq!(record.severity_number(), Some(Severity::Info));
        assert_eq!(record.body(), Some(&AnyValue::from("test.event")));
        assert_eq!(
            record.attributes_iter().collect::<Vec<_>>(),
            [&(Key::from("data"), AnyValue::from(r#"{"a":1}"#))]
        );
    }
}

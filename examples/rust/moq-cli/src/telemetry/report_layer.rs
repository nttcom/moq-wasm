use std::{
    hash::{BuildHasher, Hasher, RandomState},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use client_telemetry::{SpanRecordLayer, log_record};
use tracing::{Event, Subscriber};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use super::ReportBuffer;

/// Copies each event and each closed span it sees into `buffer`, in the log
/// and span shapes of the observability server's `/api/client-telemetry`.
pub fn report_layer<S>(buffer: ReportBuffer) -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let spans = buffer.clone();
    LogLayer(buffer).and_then(SpanRecordLayer {
        now_ms,
        random_byte: || random_u64() as u8,
        emit: move |span| spans.push_span(span),
    })
}

struct LogLayer(ReportBuffer);

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        self.0.push_log(log_record(event, now_ms()));
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// A random 64-bit value from std's randomly keyed hasher, so the CLI needs no
/// RNG crate.
fn random_u64() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::{ReportBuffer, report_layer};

    #[test]
    fn events_become_logs_and_closed_spans_become_spans() {
        // Arrange
        let buffer = ReportBuffer::default();
        let subscriber = tracing_subscriber::registry().with(report_layer(buffer.clone()));

        // Act
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("moqt.subscriber.subscribe").in_scope(|| {
                tracing::warn!("SUBSCRIBE_ERROR received");
            });
        });

        // Assert
        let (logs, spans) = buffer.take();
        let ([log], [span]) = (logs.as_slice(), spans.as_slice()) else {
            panic!("{logs:?} {spans:?}");
        };
        assert_eq!(log["message"], "SUBSCRIBE_ERROR received");
        assert_eq!(span["name"], "moqt.subscriber.subscribe");
        assert_eq!(span["traceId"].as_str().unwrap().len(), 32);
    }
}

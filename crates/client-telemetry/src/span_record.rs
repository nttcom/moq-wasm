use std::cell::RefCell;
use std::fmt::Write;

use serde_json::{Value, json};
use tracing::Subscriber;
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use crate::log_record::FieldVisitor;

/// Hands every closed span to `emit` as a span record. `now_ms` is the time
/// since the Unix epoch. Trace and span ids need to be unique, not
/// unpredictable, so `random_byte` need not be a cryptographic RNG.
pub struct SpanRecordLayer<E> {
    pub now_ms: fn() -> u64,
    pub random_byte: fn() -> u8,
    pub emit: E,
}

struct OpenSpan {
    trace_id: [u8; 16],
    span_id: [u8; 8],
    parent_span_id: Option<[u8; 8]>,
    start_ms: u64,
    fields: FieldVisitor,
}

thread_local! {
    /// The `(trace_id, span_id)` of each span entered on this thread,
    /// innermost last. An `Instrumented` future enters and exits its span on
    /// the thread that polls it, so the stack stays balanced per thread.
    static ENTERED: RefCell<Vec<([u8; 16], [u8; 8])>> = const { RefCell::new(Vec::new()) };
}

/// The innermost entered span as a W3C `traceparent`; `None` outside any span.
pub fn current_traceparent() -> Option<String> {
    let (trace_id, span_id) = current_span_ids()?;
    Some(format!("00-{}-{}-01", hex(&trace_id), hex(&span_id)))
}

pub(crate) fn current_span_ids() -> Option<([u8; 16], [u8; 8])> {
    ENTERED.with_borrow(|entered| entered.last().copied())
}

impl<E> SpanRecordLayer<E> {
    fn random<const N: usize>(&self) -> [u8; N] {
        std::array::from_fn(|_| (self.random_byte)())
    }
}

impl<S, E> Layer<S> for SpanRecordLayer<E>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    E: Fn(Value) + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let parent = span.parent().and_then(|parent| {
            let extensions = parent.extensions();
            let open = extensions.get::<OpenSpan>()?;
            Some((open.trace_id, open.span_id))
        });
        let mut fields = FieldVisitor::default();
        attrs.record(&mut fields);
        span.extensions_mut().insert(OpenSpan {
            trace_id: parent.map_or_else(|| self.random(), |(trace_id, _)| trace_id),
            span_id: self.random(),
            parent_span_id: parent.map(|(_, span_id)| span_id),
            start_ms: (self.now_ms)(),
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

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(open) = span.extensions().get::<OpenSpan>()
        {
            ENTERED.with_borrow_mut(|entered| entered.push((open.trace_id, open.span_id)));
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && span.extensions().get::<OpenSpan>().is_some()
        {
            ENTERED.with_borrow_mut(Vec::pop);
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
            "traceId": hex(&open.trace_id),
            "spanId": hex(&open.span_id),
            "name": span.name(),
            "startTimeMs": open.start_ms,
            "endTimeMs": (self.now_ms)(),
            "fields": open.fields.fields,
        });
        if let Some(parent_span_id) = open.parent_span_id {
            record["parentSpanId"] = json!(hex(&parent_span_id));
        }
        (self.emit)(record);
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, Mutex};

    use serde_json::Value;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{SpanRecordLayer, current_traceparent};

    fn capture(emit_spans: impl FnOnce()) -> Vec<Value> {
        static NEXT_BYTE: AtomicU8 = AtomicU8::new(0);
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = records.clone();
        let layer = SpanRecordLayer {
            now_ms: || 1000,
            random_byte: || NEXT_BYTE.fetch_add(1, Ordering::Relaxed),
            emit: move |record| sink.lock().unwrap().push(record),
        };
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, emit_spans);
        std::mem::take(&mut *records.lock().unwrap())
    }

    fn is_hex(value: &Value, len: usize) -> bool {
        value.as_str().is_some_and(|id| {
            id.len() == len
                && id
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    }

    #[test]
    fn a_closed_span_carries_its_name_recorded_fields_and_ids() {
        // Act
        let records = capture(|| {
            let span = tracing::info_span!(
                "moqt.subscriber.subscribe",
                track_name = "video",
                request_id = tracing::field::Empty,
            );
            span.record("request_id", 4u64);
        });

        // Assert
        let [record] = records.as_slice() else {
            panic!("{records:?}");
        };
        assert_eq!(record["name"], "moqt.subscriber.subscribe");
        assert_eq!(record["fields"]["track_name"], "video");
        assert_eq!(record["fields"]["request_id"], 4);
        assert_eq!(record["startTimeMs"], 1000);
        assert_eq!(record["endTimeMs"], 1000);
        assert!(is_hex(&record["traceId"], 32), "{record}");
        assert!(is_hex(&record["spanId"], 16), "{record}");
        assert!(record.get("parentSpanId").is_none());
    }

    #[test]
    fn a_child_span_shares_its_parent_s_trace() {
        // Act
        let records = capture(|| {
            let _parent = tracing::info_span!("parent").entered();
            tracing::info_span!("child").in_scope(|| {});
        });

        // Assert
        let [child, parent] = records.as_slice() else {
            panic!("{records:?}");
        };
        assert_eq!(child["traceId"], parent["traceId"]);
        assert_eq!(child["parentSpanId"], parent["spanId"]);
        assert_ne!(child["spanId"], parent["spanId"]);
    }

    #[test]
    fn inside_an_entered_span_the_traceparent_names_that_span() {
        // Arrange
        let mut traceparent = None;

        // Act
        let records = capture(|| {
            traceparent = tracing::info_span!("request").in_scope(current_traceparent);
        });

        // Assert
        let [record] = records.as_slice() else {
            panic!("{records:?}");
        };
        let expected = format!(
            "00-{}-{}-01",
            record["traceId"].as_str().unwrap(),
            record["spanId"].as_str().unwrap()
        );
        assert_eq!(traceparent, Some(expected));
    }

    #[test]
    fn outside_any_span_there_is_no_traceparent() {
        // Act
        let mut traceparent = Some(String::new());
        capture(|| {
            tracing::info_span!("closed").in_scope(|| {});
            traceparent = current_traceparent();
        });

        // Assert
        assert_eq!(traceparent, None);
    }
}

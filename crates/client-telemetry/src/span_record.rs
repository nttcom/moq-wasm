use std::cell::RefCell;
use std::fmt::Write;

use serde_json::{Value, json};
use tracing::span::{Attributes, Id, Record};
use tracing::{Span, Subscriber};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use crate::log_record::FieldVisitor;
use crate::scope::current_scope;

/// Hands every closed span to `emit` as a span record, with its scope: its
/// parent's, else `current_scope()` where it was opened. `now_ms` is the time
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
    scope: Option<u64>,
    fields: FieldVisitor,
}

#[derive(Clone, Copy)]
struct EnteredSpan {
    trace_id: [u8; 16],
    span_id: [u8; 8],
    scope: Option<u64>,
}

thread_local! {
    /// Each span entered on this thread, innermost last. An `Instrumented` future enters and exits its span on
    /// the thread that polls it, so the stack stays balanced per thread.
    static ENTERED: RefCell<Vec<EnteredSpan>> = const { RefCell::new(Vec::new()) };
}

/// The innermost entered span as a W3C `traceparent`; `None` outside any span.
pub fn current_traceparent() -> Option<String> {
    let (trace_id, span_id) = current_span_ids()?;
    Some(format!("00-{}-{}-01", hex(&trace_id), hex(&span_id)))
}

/// Makes `span` a child of the remote span a W3C `traceparent` names. Call it
/// before `span` is first entered. A value that is not a version-00
/// traceparent, or a subscriber other than a `Registry` carrying
/// `SpanRecordLayer`, leaves `span` as it is.
pub fn continue_trace_from_traceparent(span: &Span, traceparent: &str) {
    let Some((trace_id, parent_span_id)) = parse_traceparent(traceparent) else {
        return;
    };
    span.with_subscriber(|(id, dispatch)| {
        let Some(span) = dispatch
            .downcast_ref::<Registry>()
            .and_then(|registry| registry.span(id))
        else {
            return;
        };
        if let Some(open) = span.extensions_mut().get_mut::<OpenSpan>() {
            open.trace_id = trace_id;
            open.parent_span_id = Some(parent_span_id);
        }
    });
}

/// `00-<trace id>-<parent id>-<flags>` with ids that are not all zero
/// (W3C Trace Context §3.2).
fn parse_traceparent(traceparent: &str) -> Option<([u8; 16], [u8; 8])> {
    let ["00", trace_id, parent_id, flags] = traceparent.split('-').collect::<Vec<_>>()[..] else {
        return None;
    };
    lowercase_hex(flags, 2)?;
    let trace_id = lowercase_hex(trace_id, 32).filter(|id| *id != 0)?;
    let parent_id = lowercase_hex(parent_id, 16).filter(|id| *id != 0)? as u64;
    Some((trace_id.to_be_bytes(), parent_id.to_be_bytes()))
}

fn lowercase_hex(text: &str, digits: usize) -> Option<u128> {
    let valid = text.len() == digits
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    valid.then(|| u128::from_str_radix(text, 16).ok())?
}

pub(crate) fn current_span_ids() -> Option<([u8; 16], [u8; 8])> {
    ENTERED.with_borrow(|entered| {
        entered
            .last()
            .map(|entered| (entered.trace_id, entered.span_id))
    })
}

pub(crate) fn entered_span_scope() -> Option<u64> {
    ENTERED.with_borrow(|entered| entered.last().and_then(|entered| entered.scope))
}

impl<E> SpanRecordLayer<E> {
    fn random<const N: usize>(&self) -> [u8; N] {
        std::array::from_fn(|_| (self.random_byte)())
    }
}

impl<S, E> Layer<S> for SpanRecordLayer<E>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    E: Fn(Value, Option<u64>) + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let parent = span.parent().and_then(|parent| {
            let extensions = parent.extensions();
            let open = extensions.get::<OpenSpan>()?;
            Some((open.trace_id, open.span_id, open.scope))
        });
        let mut fields = FieldVisitor::default();
        attrs.record(&mut fields);
        span.extensions_mut().insert(OpenSpan {
            trace_id: parent.map_or_else(|| self.random(), |(trace_id, ..)| trace_id),
            span_id: self.random(),
            parent_span_id: parent.map(|(_, span_id, _)| span_id),
            start_ms: (self.now_ms)(),
            scope: parent.and_then(|(.., scope)| scope).or_else(current_scope),
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
            ENTERED.with_borrow_mut(|entered| {
                entered.push(EnteredSpan {
                    trace_id: open.trace_id,
                    span_id: open.span_id,
                    scope: open.scope,
                });
            });
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
        (self.emit)(record, open.scope);
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

    use super::{SpanRecordLayer, continue_trace_from_traceparent, current_traceparent};
    use crate::{current_scope, in_scope};

    fn capture(emit_spans: impl FnOnce()) -> Vec<Value> {
        capture_with_scopes(emit_spans)
            .into_iter()
            .map(|(record, _)| record)
            .collect()
    }

    fn capture_with_scopes(emit_spans: impl FnOnce()) -> Vec<(Value, Option<u64>)> {
        static NEXT_BYTE: AtomicU8 = AtomicU8::new(0);
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink = records.clone();
        let layer = SpanRecordLayer {
            now_ms: || 1000,
            random_byte: || NEXT_BYTE.fetch_add(1, Ordering::Relaxed),
            emit: move |record, scope| sink.lock().unwrap().push((record, scope)),
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

    const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_span_continued_from_a_traceparent_joins_its_trace_with_its_children() {
        // Act
        let records = capture(|| {
            let answer = tracing::info_span!(parent: None, "answer");
            continue_trace_from_traceparent(&answer, TRACEPARENT);
            answer.in_scope(|| tracing::info_span!("child").in_scope(|| {}));
        });

        // Assert
        let [child, answer] = records.as_slice() else {
            panic!("{records:?}");
        };
        assert_eq!(answer["traceId"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(answer["parentSpanId"], "00f067aa0ba902b7");
        assert_eq!(child["traceId"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(child["parentSpanId"], answer["spanId"]);
    }

    #[test]
    fn a_value_that_is_not_a_traceparent_leaves_the_span_a_trace_root() {
        // Arrange
        let invalid = [
            "",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-00",
        ];

        for traceparent in invalid {
            // Act
            let records = capture(|| {
                let answer = tracing::info_span!(parent: None, "answer");
                continue_trace_from_traceparent(&answer, traceparent);
            });

            // Assert
            let [answer] = records.as_slice() else {
                panic!("{records:?}");
            };
            assert!(answer.get("parentSpanId").is_none(), "{traceparent}");
            assert_ne!(answer["traceId"], "4bf92f3577b34da6a3ce929d0e0e4736");
        }
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

    #[test]
    fn a_span_opened_in_a_scope_passes_it_on_to_its_children_and_events() {
        // Arrange
        let mut event_scope = None;

        // Act
        let records = capture_with_scopes(|| {
            let connect = in_scope(5, || tracing::info_span!("moqt.session.connect"));
            connect.in_scope(|| {
                tracing::info_span!("child").in_scope(|| event_scope = current_scope());
            });
        });

        // Assert: the child was opened and entered outside `in_scope`
        let scopes: Vec<_> = records.iter().map(|(_, scope)| *scope).collect();
        assert_eq!(scopes, vec![Some(5), Some(5)]);
        assert_eq!(event_scope, Some(5));
    }

    #[test]
    fn a_span_opened_outside_any_scope_has_none() {
        // Act
        let records = capture_with_scopes(|| tracing::info_span!("request").in_scope(|| {}));

        // Assert
        let [(_, scope)] = records.as_slice() else {
            panic!("{records:?}");
        };
        assert_eq!(*scope, None);
    }
}

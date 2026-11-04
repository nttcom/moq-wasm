use std::cell::RefCell;

use client_telemetry::{SpanRecordLayer, current_traceparent, log_record};
use js_sys::Function;
use serde_json::Value;
use tracing::level_filters::LevelFilter;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use wasm_bindgen::prelude::*;

use crate::js_error;

thread_local! {
    static LOG_CALLBACK: RefCell<Option<Function>> = const { RefCell::new(None) };
    static SPAN_CALLBACK: RefCell<Option<Function>> = const { RefCell::new(None) };
}

/// Installs the page-wide `tracing` subscriber that hands every event at
/// `level` (`error`, `warn`, `info`, `debug` or `trace`) or above to `callback`
/// as a `LogRecord`, and, with `span_callback`, every closed INFO or higher span
/// as a `SpanRecord`. Fails when the level is unknown or a sink is already set:
/// the subscriber is global and can be installed once per page.
#[wasm_bindgen(js_name = setLogSink)]
pub fn set_log_sink(
    callback: Function,
    level: &str,
    span_callback: Option<Function>,
) -> Result<(), JsValue> {
    let level: LevelFilter = level
        .parse()
        .map_err(|_| js_error(format!("unknown log level: {level}")))?;
    let span_layer = span_callback.is_some().then(|| {
        SpanRecordLayer {
            now_ms,
            random_byte: || (js_sys::Math::random() * 256.0) as u8,
            emit: |record| call(&SPAN_CALLBACK, &record),
        }
        .with_filter(LevelFilter::INFO)
    });
    let subscriber = tracing_subscriber::registry()
        .with(JsLogLayer.with_filter(level))
        .with(span_layer);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| js_error("the log sink is already set"))?;
    LOG_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
    if span_callback.is_some() {
        moqt::set_trace_context_provider(current_traceparent);
    }
    SPAN_CALLBACK.with(|slot| *slot.borrow_mut() = span_callback);
    Ok(())
}

fn call(callback: &'static std::thread::LocalKey<RefCell<Option<Function>>>, record: &Value) {
    let Ok(record) = js_sys::JSON::parse(&record.to_string()) else {
        return;
    };
    callback.with(|slot| {
        if let Some(callback) = slot.borrow().as_ref() {
            // A throwing callback must not break the code that logged.
            let _ = callback.call1(&JsValue::NULL, &record);
        }
    });
}

struct JsLogLayer;

impl<S: Subscriber> Layer<S> for JsLogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        call(&LOG_CALLBACK, &log_record(event, now_ms()));
    }
}

fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

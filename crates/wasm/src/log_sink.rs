use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use client_telemetry::{
    Scoped, SpanRecordLayer, current_scope, current_traceparent, in_scope, log_record, scoped,
};
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
    /// The newest token of each `SessionScope`.
    static SCOPE_TOKENS: RefCell<HashMap<u64, String>> = RefCell::new(HashMap::new());
    static NEXT_SCOPE: Cell<u64> = const { Cell::new(0) };
}

/// The spans and logs of one session, which the callbacks receive with the
/// session's newest token.
#[derive(Clone, Copy)]
pub(crate) struct SessionScope(u64);

impl SessionScope {
    pub(crate) fn new() -> Self {
        let scope = NEXT_SCOPE.get();
        NEXT_SCOPE.set(scope + 1);
        Self(scope)
    }

    pub(crate) fn set_token(self, token: String) {
        SCOPE_TOKENS.with_borrow_mut(|tokens| tokens.insert(self.0, token));
    }

    pub(crate) fn token(self) -> Option<String> {
        SCOPE_TOKENS.with_borrow(|tokens| tokens.get(&self.0).cloned())
    }

    pub(crate) fn scoped<F: Future>(self, future: F) -> Scoped<F> {
        scoped(self.0, future)
    }

    pub(crate) fn in_scope<R>(self, f: impl FnOnce() -> R) -> R {
        in_scope(self.0, f)
    }
}

fn token_of(scope: u64) -> JsValue {
    SCOPE_TOKENS.with_borrow(|tokens| {
        tokens
            .get(&scope)
            .map_or(JsValue::UNDEFINED, |token| JsValue::from_str(token))
    })
}

fn carry_scope(future: Pin<Box<dyn Future<Output = ()>>>) -> Pin<Box<dyn Future<Output = ()>>> {
    match current_scope() {
        Some(scope) => Box::pin(scoped(scope, future)),
        None => future,
    }
}

/// Installs the page-wide `tracing` subscriber that hands every event at
/// `level` (`error`, `warn`, `info`, `debug` or `trace`) or above to `callback`
/// as a `LogRecord`, and, with `span_callback`, every closed INFO or higher span
/// as a `SpanRecord`. Both take the token of the record's session as their
/// second argument, `undefined` when the record belongs to no session or the
/// session has no token. Fails when the level is unknown or a sink is already set:
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
            emit: |record, scope: Option<u64>| {
                call(
                    &SPAN_CALLBACK,
                    &record,
                    &scope.map_or(JsValue::UNDEFINED, token_of),
                );
            },
        }
        .with_filter(LevelFilter::INFO)
    });
    let subscriber = tracing_subscriber::registry()
        .with(JsLogLayer.with_filter(level))
        .with(span_layer);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| js_error("the log sink is already set"))?;
    LOG_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
    moqt::executor::set_spawn_wrapper(carry_scope);
    if span_callback.is_some() {
        moqt::set_trace_context_provider(current_traceparent);
    }
    SPAN_CALLBACK.with(|slot| *slot.borrow_mut() = span_callback);
    Ok(())
}

fn call(
    callback: &'static std::thread::LocalKey<RefCell<Option<Function>>>,
    record: &Value,
    token: &JsValue,
) {
    let Ok(record) = js_sys::JSON::parse(&record.to_string()) else {
        return;
    };
    callback.with(|slot| {
        if let Some(callback) = slot.borrow().as_ref() {
            // A throwing callback must not break the code that logged.
            let _ = callback.call2(&JsValue::NULL, &record, token);
        }
    });
}

struct JsLogLayer;

impl<S: Subscriber> Layer<S> for JsLogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        call(
            &LOG_CALLBACK,
            &log_record(event, now_ms()),
            &current_scope().map_or(JsValue::UNDEFINED, token_of),
        );
    }
}

fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

use std::cell::RefCell;

use js_sys::Function;
use tracing::level_filters::LevelFilter;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use wasm_bindgen::prelude::*;

use crate::js_error;
use crate::log_record::log_record;

thread_local! {
    static LOG_CALLBACK: RefCell<Option<Function>> = const { RefCell::new(None) };
}

/// Installs the page-wide `tracing` subscriber that hands every event at
/// `level` (`error`, `warn`, `info`, `debug` or `trace`) or above to `callback`
/// as a `LogRecord`. Fails when the level is unknown or a sink is already set:
/// the subscriber is global and can be installed once per page.
#[wasm_bindgen(js_name = setLogSink)]
pub fn set_log_sink(callback: Function, level: &str) -> Result<(), JsValue> {
    let level: LevelFilter = level
        .parse()
        .map_err(|_| js_error(format!("unknown log level: {level}")))?;
    let subscriber = tracing_subscriber::registry().with(level).with(JsLogLayer);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| js_error("the log sink is already set"))?;
    LOG_CALLBACK.with(|slot| *slot.borrow_mut() = Some(callback));
    Ok(())
}

struct JsLogLayer;

impl<S: Subscriber> Layer<S> for JsLogLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let record = log_record(event, js_sys::Date::now());
        let Ok(record) = js_sys::JSON::parse(&record.to_string()) else {
            return;
        };
        LOG_CALLBACK.with(|slot| {
            if let Some(callback) = slot.borrow().as_ref() {
                // A throwing callback must not break the code that logged.
                let _ = callback.call1(&JsValue::NULL, &record);
            }
        });
    }
}

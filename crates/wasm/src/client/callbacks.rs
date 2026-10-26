use std::cell::RefCell;

use js_sys::Function;
use wasm_bindgen::JsValue;

#[derive(Default)]
pub(crate) struct Callbacks {
    pub(crate) server_setup: Option<Function>,
    pub(crate) publish_namespace: Option<Function>,
    pub(crate) publish_namespace_done: Option<Function>,
    pub(crate) publish_namespace_response: Option<Function>,
    pub(crate) subscribe_namespace_response: Option<Function>,
    pub(crate) publish: Option<Function>,
    pub(crate) publish_response: Option<Function>,
    pub(crate) subscribe: Option<Function>,
    pub(crate) subscribe_response: Option<Function>,
    pub(crate) incoming_unsubscribe: Option<Function>,
    pub(crate) publish_done: Option<Function>,
    pub(crate) object_datagram: Option<Function>,
    pub(crate) object_datagram_status: Option<Function>,
    pub(crate) subgroup_header: Option<Function>,
    pub(crate) subgroup_object: Option<Function>,
    pub(crate) fetch: Option<Function>,
    pub(crate) fetch_cancel: Option<Function>,
    pub(crate) fetch_response: Option<Function>,
    pub(crate) fetch_object: Option<Function>,
    pub(crate) fetch_stream_end: Option<Function>,
    pub(crate) track_status: Option<Function>,
    pub(crate) track_status_response: Option<Function>,
    pub(crate) connection_closed: Option<Function>,
}

pub(crate) type CallbackSelector = fn(&Callbacks) -> &Option<Function>;

/// The callback is cloned out of the cell before it runs so that a JavaScript
/// handler may register or clear callbacks re-entrantly.
pub(crate) fn emit(callbacks: &RefCell<Callbacks>, select: CallbackSelector, args: &[JsValue]) {
    let callback = select(&callbacks.borrow()).clone();
    let Some(callback) = callback else {
        return;
    };
    let _ = callback.apply(&JsValue::NULL, &args.iter().collect::<js_sys::Array>());
}

pub(crate) fn has_callback(callbacks: &RefCell<Callbacks>, select: CallbackSelector) -> bool {
    select(&callbacks.borrow()).is_some()
}

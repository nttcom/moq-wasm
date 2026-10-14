use std::cell::RefCell;

use js_sys::Function;
use mediapack::loc::from_extension_headers;
use moqt::{
    object::{Object, SubgroupStart},
    wire::FetchObjectField,
};
use wasm_bindgen::JsValue;

use crate::messages::{
    FetchObjectMessage, FetchStreamEndMessage, ObjectDatagramMessage, ObjectDatagramStatusMessage,
    SubgroupHeaderMessage, SubgroupObjectMessage,
};

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

pub(crate) fn emit_object_datagram(
    callbacks: &RefCell<Callbacks>,
    track_alias: u64,
    object: Object,
) {
    let loc_header = from_extension_headers(&object.extension_headers);
    let object_id = Some(object.location.object_id);
    if object.payload.is_empty() {
        let message = ObjectDatagramStatusMessage::new(
            track_alias,
            object.location.group_id,
            object_id,
            object.publisher_priority,
            object.status,
            loc_header,
        );
        emit(callbacks, |c| &c.object_datagram_status, &[message.into()]);
    } else {
        let message = ObjectDatagramMessage::new(
            track_alias,
            object.location.group_id,
            object_id,
            object.publisher_priority,
            object.payload.to_vec(),
            loc_header,
        );
        emit(callbacks, |c| &c.object_datagram, &[message.into()]);
    }
}

pub(crate) fn emit_subgroup_header(
    callbacks: &RefCell<Callbacks>,
    track_alias: u64,
    start: &SubgroupStart,
) {
    let message = SubgroupHeaderMessage::new(
        track_alias,
        start.group_id,
        start.subgroup_id,
        start.publisher_priority,
    );
    emit(callbacks, |c| &c.subgroup_header, &[message.into()]);
}

pub(crate) fn emit_subgroup_object(
    callbacks: &RefCell<Callbacks>,
    track_alias: u64,
    object: Object,
) {
    let loc_header = from_extension_headers(&object.extension_headers);
    let group_id = object.location.group_id;
    let message = SubgroupObjectMessage::new(object, loc_header);
    emit(
        callbacks,
        |c| &c.subgroup_object,
        &[
            js_sys::BigInt::from(track_alias).into(),
            js_sys::BigInt::from(group_id).into(),
            message.into(),
        ],
    );
}

pub(crate) fn emit_fetch_object(
    callbacks: &RefCell<Callbacks>,
    request_id: u64,
    field: &FetchObjectField,
) {
    let message = FetchObjectMessage::new(request_id, field);
    emit(callbacks, |c| &c.fetch_object, &[message.into()]);
}

pub(crate) fn emit_fetch_stream_end(
    callbacks: &RefCell<Callbacks>,
    message: FetchStreamEndMessage,
) {
    emit(callbacks, |c| &c.fetch_stream_end, &[message.into()]);
}

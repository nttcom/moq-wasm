use std::cell::RefCell;

use js_sys::Function;
use mediapack::loc::from_extension_headers;
use moqt::wire::{
    FetchObjectField, ObjectDatagram, ObjectDatagramPayload, SubgroupHeader, SubgroupId,
    SubgroupObjectField,
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

pub(crate) fn emit_object_datagram(callbacks: &RefCell<Callbacks>, datagram: ObjectDatagram) {
    let field = datagram.field;
    let loc_header = field
        .extension_headers
        .as_ref()
        .map(from_extension_headers)
        .unwrap_or_default();
    match field.payload {
        ObjectDatagramPayload::Payload(payload) => {
            let message = ObjectDatagramMessage::new(
                datagram.track_alias,
                datagram.group_id,
                field.object_id,
                field.publisher_priority,
                payload.to_vec(),
                loc_header,
            );
            emit(callbacks, |c| &c.object_datagram, &[message.into()]);
        }
        ObjectDatagramPayload::Status(status) => {
            let message = ObjectDatagramStatusMessage::new(
                datagram.track_alias,
                datagram.group_id,
                field.object_id,
                field.publisher_priority,
                status,
                loc_header,
            );
            emit(callbacks, |c| &c.object_datagram_status, &[message.into()]);
        }
    }
}

fn subgroup_id_value(header: &SubgroupHeader) -> Option<u64> {
    match header.subgroup_id {
        SubgroupId::Value(value) => Some(value),
        _ => None,
    }
}

pub(crate) fn emit_subgroup_header(callbacks: &RefCell<Callbacks>, header: &SubgroupHeader) {
    let message = SubgroupHeaderMessage::new(
        header.track_alias,
        header.group_id,
        subgroup_id_value(header),
        header.publisher_priority,
    );
    emit(callbacks, |c| &c.subgroup_header, &[message.into()]);
}

pub(crate) fn emit_subgroup_object(
    callbacks: &RefCell<Callbacks>,
    header: &SubgroupHeader,
    field: SubgroupObjectField,
    object_id: u64,
) {
    let loc_header = from_extension_headers(&field.extension_headers);
    let message =
        SubgroupObjectMessage::new(subgroup_id_value(header), object_id, field, loc_header);
    emit(
        callbacks,
        |c| &c.subgroup_object,
        &[
            js_sys::BigInt::from(header.track_alias).into(),
            js_sys::BigInt::from(header.group_id).into(),
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

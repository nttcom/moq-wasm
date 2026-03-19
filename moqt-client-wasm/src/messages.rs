//! JavaScript-friendly wrapper types for MoQT messages
//!
//! This module contains wasm-bindgen compatible wrapper types that expose
//! moqt-core message types to JavaScript/TypeScript with proper type definitions.
mod subgroup_state;
use moqt_core::messages::control_messages::{
    announce::Announce, announce_error::AnnounceError, announce_ok::AnnounceOk,
    server_setup::ServerSetup, subscribe::Subscribe,
    subscribe_announces_error::SubscribeAnnouncesError,
    subscribe_announces_ok::SubscribeAnnouncesOk, subscribe_done::SubscribeDone,
    subscribe_error::SubscribeError, subscribe_ok::SubscribeOk,
};
use moqt_core::messages::data_streams::{
    object_status::ObjectStatus, subgroup_stream::Object as CoreSubgroupStreamObject,
};
use packages::loc::LocHeader;
pub use subgroup_state::SubgroupState;
use wasm_bindgen::prelude::*;

/// JavaScript-friendly wrapper for Announce message
#[wasm_bindgen]
#[derive(Clone)]
pub struct AnnounceMessage {
    track_namespace: Vec<String>,
}

#[wasm_bindgen]
impl AnnounceMessage {
    /// Get track namespace as a JavaScript array
    #[wasm_bindgen(getter, js_name = trackNamespace)]
    pub fn track_namespace(&self) -> Vec<String> {
        self.track_namespace.clone()
    }
}

impl From<&Announce> for AnnounceMessage {
    fn from(announce: &Announce) -> Self {
        AnnounceMessage {
            track_namespace: announce.track_namespace().clone(),
        }
    }
}

/// JavaScript-friendly wrapper for ServerSetup message
#[wasm_bindgen]
#[derive(Clone)]
pub struct ServerSetupMessage {
    version: u32,
}

#[wasm_bindgen]
impl ServerSetupMessage {
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> u32 {
        self.version
    }
}

impl From<&ServerSetup> for ServerSetupMessage {
    fn from(setup: &ServerSetup) -> Self {
        ServerSetupMessage {
            version: setup.selected_version,
        }
    }
}

/// JavaScript-friendly wrapper for AnnounceOk message
#[wasm_bindgen]
#[derive(Clone)]
pub struct AnnounceOkMessage {
    track_namespace: Vec<String>,
}

#[wasm_bindgen]
impl AnnounceOkMessage {
    #[wasm_bindgen(getter, js_name = trackNamespace)]
    pub fn track_namespace(&self) -> Vec<String> {
        self.track_namespace.clone()
    }
}

impl From<&AnnounceOk> for AnnounceOkMessage {
    fn from(announce_ok: &AnnounceOk) -> Self {
        AnnounceOkMessage {
            track_namespace: announce_ok.track_namespace().clone(),
        }
    }
}

/// JavaScript-friendly wrapper for AnnounceError message
#[wasm_bindgen]
#[derive(Clone)]
pub struct AnnounceErrorMessage {
    track_namespace: Vec<String>,
    error_code: u64,
    reason_phrase: String,
}

#[wasm_bindgen]
impl AnnounceErrorMessage {
    #[wasm_bindgen(getter, js_name = trackNamespace)]
    pub fn track_namespace(&self) -> Vec<String> {
        self.track_namespace.clone()
    }

    #[wasm_bindgen(getter, js_name = errorCode)]
    pub fn error_code(&self) -> u64 {
        self.error_code
    }

    #[wasm_bindgen(getter, js_name = reasonPhrase)]
    pub fn reason_phrase(&self) -> String {
        self.reason_phrase.clone()
    }
}

impl From<&AnnounceError> for AnnounceErrorMessage {
    fn from(announce_error: &AnnounceError) -> Self {
        // Serialize to get all fields including private ones
        let serialized = serde_wasm_bindgen::to_value(announce_error).unwrap();
        let reason_phrase = js_sys::Reflect::get(&serialized, &JsValue::from_str("reason_phrase"))
            .unwrap()
            .as_string()
            .unwrap_or_default();

        AnnounceErrorMessage {
            track_namespace: announce_error.track_namespace().clone(),
            error_code: announce_error.error_code(),
            reason_phrase,
        }
    }
}

/// JavaScript-friendly wrapper for Subscribe message
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeMessage {
    subscribe_id: u64,
    track_alias: u64,
    track_namespace: Vec<String>,
    track_name: String,
}

#[wasm_bindgen]
impl SubscribeMessage {
    #[wasm_bindgen(getter, js_name = subscribeId)]
    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    #[wasm_bindgen(getter, js_name = trackAlias)]
    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    #[wasm_bindgen(getter, js_name = trackNamespace)]
    pub fn track_namespace(&self) -> Vec<String> {
        self.track_namespace.clone()
    }

    #[wasm_bindgen(getter, js_name = trackName)]
    pub fn track_name(&self) -> String {
        self.track_name.clone()
    }
}

impl From<&Subscribe> for SubscribeMessage {
    fn from(subscribe: &Subscribe) -> Self {
        SubscribeMessage {
            subscribe_id: subscribe.subscribe_id(),
            track_alias: subscribe.track_alias(),
            track_namespace: subscribe.track_namespace().to_vec(),
            track_name: subscribe.track_name().to_string(),
        }
    }
}

/// JavaScript-friendly wrapper for SubscribeAnnouncesOk message
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeAnnouncesOkMessage {
    track_namespace_prefix: Vec<String>,
}

#[wasm_bindgen]
impl SubscribeAnnouncesOkMessage {
    #[wasm_bindgen(getter, js_name = trackNamespacePrefix)]
    pub fn track_namespace_prefix(&self) -> Vec<String> {
        self.track_namespace_prefix.clone()
    }
}

impl From<&SubscribeAnnouncesOk> for SubscribeAnnouncesOkMessage {
    fn from(subscribe_announces_ok: &SubscribeAnnouncesOk) -> Self {
        SubscribeAnnouncesOkMessage {
            track_namespace_prefix: subscribe_announces_ok.track_namespace_prefix().clone(),
        }
    }
}

/// JavaScript-friendly wrapper for SubscribeAnnouncesError message
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeAnnouncesErrorMessage {
    track_namespace_prefix: Vec<String>,
    error_code: u64,
    reason_phrase: String,
}

#[wasm_bindgen]
impl SubscribeAnnouncesErrorMessage {
    #[wasm_bindgen(getter, js_name = trackNamespacePrefix)]
    pub fn track_namespace_prefix(&self) -> Vec<String> {
        self.track_namespace_prefix.clone()
    }

    #[wasm_bindgen(getter, js_name = errorCode)]
    pub fn error_code(&self) -> u64 {
        self.error_code
    }

    #[wasm_bindgen(getter, js_name = reasonPhrase)]
    pub fn reason_phrase(&self) -> String {
        self.reason_phrase.clone()
    }
}

impl From<&SubscribeAnnouncesError> for SubscribeAnnouncesErrorMessage {
    fn from(subscribe_announces_error: &SubscribeAnnouncesError) -> Self {
        // Serialize to get all fields including private ones
        let serialized = serde_wasm_bindgen::to_value(subscribe_announces_error).unwrap();
        let reason_phrase = js_sys::Reflect::get(&serialized, &JsValue::from_str("reason_phrase"))
            .unwrap()
            .as_string()
            .unwrap_or_default();

        SubscribeAnnouncesErrorMessage {
            track_namespace_prefix: subscribe_announces_error.track_namespace_prefix().clone(),
            error_code: subscribe_announces_error.error_code(),
            reason_phrase,
        }
    }
}

/// JavaScript-friendly wrapper for SubscribeOk message
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeOkMessage {
    subscribe_id: u64,
}

#[wasm_bindgen]
impl SubscribeOkMessage {
    #[wasm_bindgen(getter, js_name = subscribeId)]
    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }
}

impl From<&SubscribeOk> for SubscribeOkMessage {
    fn from(subscribe_ok: &SubscribeOk) -> Self {
        SubscribeOkMessage {
            subscribe_id: subscribe_ok.subscribe_id(),
        }
    }
}

/// JavaScript-friendly wrapper for SubscribeError message
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeErrorMessage {
    subscribe_id: u64,
    error_code: u64,
    reason_phrase: String,
}

#[wasm_bindgen]
impl SubscribeErrorMessage {
    #[wasm_bindgen(getter, js_name = subscribeId)]
    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    #[wasm_bindgen(getter, js_name = errorCode)]
    pub fn error_code(&self) -> u64 {
        self.error_code
    }

    #[wasm_bindgen(getter, js_name = reasonPhrase)]
    pub fn reason_phrase(&self) -> String {
        self.reason_phrase.clone()
    }
}

impl From<&SubscribeError> for SubscribeErrorMessage {
    fn from(subscribe_error: &SubscribeError) -> Self {
        // Serialize to get all fields including private ones
        let serialized = serde_wasm_bindgen::to_value(subscribe_error).unwrap();
        let reason_phrase = js_sys::Reflect::get(&serialized, &JsValue::from_str("reason_phrase"))
            .unwrap()
            .as_string()
            .unwrap_or_default();

        SubscribeErrorMessage {
            subscribe_id: subscribe_error.subscribe_id(),
            error_code: u8::from(subscribe_error.error_code()) as u64,
            reason_phrase,
        }
    }
}

/// JavaScript-friendly wrapper for SubscribeDone message
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeDoneMessage {
    subscribe_id: u64,
    status_code: u64,
    reason_phrase: String,
    content_exists: bool,
    final_group_id: Option<u64>,
    final_object_id: Option<u64>,
}

#[wasm_bindgen]
impl SubscribeDoneMessage {
    #[wasm_bindgen(getter, js_name = subscribeId)]
    pub fn subscribe_id(&self) -> u64 {
        self.subscribe_id
    }

    #[wasm_bindgen(getter, js_name = statusCode)]
    pub fn status_code(&self) -> u64 {
        self.status_code
    }

    #[wasm_bindgen(getter, js_name = reasonPhrase)]
    pub fn reason_phrase(&self) -> String {
        self.reason_phrase.clone()
    }

    #[wasm_bindgen(getter, js_name = contentExists)]
    pub fn content_exists(&self) -> bool {
        self.content_exists
    }

    #[wasm_bindgen(getter, js_name = finalGroupId)]
    pub fn final_group_id(&self) -> Option<u64> {
        self.final_group_id
    }

    #[wasm_bindgen(getter, js_name = finalObjectId)]
    pub fn final_object_id(&self) -> Option<u64> {
        self.final_object_id
    }
}

impl From<&SubscribeDone> for SubscribeDoneMessage {
    fn from(subscribe_done: &SubscribeDone) -> Self {
        SubscribeDoneMessage {
            subscribe_id: subscribe_done.subscribe_id(),
            status_code: u64::from(subscribe_done.status_code()),
            reason_phrase: subscribe_done.reason_phrase().to_string(),
            content_exists: subscribe_done.content_exists(),
            final_group_id: subscribe_done.final_group_id(),
            final_object_id: subscribe_done.final_object_id(),
        }
    }
}

/// JavaScript-friendly wrapper for Subgroup Stream object message
#[wasm_bindgen]
#[derive(Clone)]
pub struct SubgroupStreamObjectMessage {
    object_id: u64,
    object_status: Option<u8>,
    object_payload_length: u32,
    object_payload: Vec<u8>,
    loc_header: LocHeader,
}

#[wasm_bindgen]
impl SubgroupStreamObjectMessage {
    #[wasm_bindgen(getter, js_name = objectId)]
    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    #[wasm_bindgen(getter, js_name = objectStatus)]
    pub fn object_status(&self) -> Option<u8> {
        self.object_status
    }

    #[wasm_bindgen(getter, js_name = objectPayload)]
    pub fn object_payload(&self) -> Vec<u8> {
        self.object_payload.clone()
    }

    #[wasm_bindgen(getter, js_name = objectPayloadLength)]
    pub fn object_payload_length(&self) -> u32 {
        self.object_payload_length
    }

    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        crate::loc::encode_loc_header(&self.loc_header)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

impl From<&CoreSubgroupStreamObject> for SubgroupStreamObjectMessage {
    fn from(value: &CoreSubgroupStreamObject) -> Self {
        let loc_header = crate::loc::extension_headers_to_loc_header(value.extension_headers());
        SubgroupStreamObjectMessage {
            object_id: value.object_id(),
            object_status: value.object_status().map(ObjectStatus::into),
            object_payload_length: value.object_payload().len() as u32,
            object_payload: value.object_payload().to_vec(),
            loc_header,
        }
    }
}

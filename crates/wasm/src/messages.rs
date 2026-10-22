mod subgroup_state;

use mediapack::loc::{LocExtension, from_extension_headers};
use moqt::{
    object::{ForwardingPreference, Object},
    wire::{
        ContentExists, FilterType, ObjectStatus, Publish, PublishOk, ServerSetup, Subscribe,
        SubscribeNamespace, SubscribeOk,
    },
};
pub use subgroup_state::SubgroupState;

use crate::client_state::IncomingFetchRequest;
use wasm_bindgen::prelude::*;

fn filter_fields(filter_type: FilterType) -> (u8, Option<u64>, Option<u64>, Option<u64>) {
    match filter_type {
        FilterType::NextGroupStart => (1, None, None, None),
        FilterType::LargestObject => (2, None, None, None),
        FilterType::AbsoluteStart { location } => {
            (3, Some(location.group_id), Some(location.object_id), None)
        }
        FilterType::AbsoluteRange {
            location,
            end_group,
        } => (
            4,
            Some(location.group_id),
            Some(location.object_id),
            Some(end_group),
        ),
    }
}

fn content_exists_fields(content_exists: ContentExists) -> (bool, Option<u64>, Option<u64>) {
    match content_exists {
        ContentExists::False => (false, None, None),
        ContentExists::True { location } => {
            (true, Some(location.group_id), Some(location.object_id))
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct ServerSetupMessage {
    #[wasm_bindgen(readonly)]
    pub version: u32,
    #[wasm_bindgen(readonly, js_name = maxRequestId)]
    pub max_request_id: u64,
}

impl From<&ServerSetup> for ServerSetupMessage {
    fn from(setup: &ServerSetup) -> Self {
        Self {
            version: setup.selected_version,
            max_request_id: setup.setup_parameters.max_request_id,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct PublishNamespaceMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = trackNamespace)]
    pub track_namespace: Vec<String>,
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct PublishNamespaceDoneMessage {
    #[wasm_bindgen(getter_with_clone, readonly, js_name = trackNamespace)]
    pub track_namespace: Vec<String>,
}

/// PUBLISH_DONE for an outgoing subscription. `request_id` is the id
/// JavaScript chose for the SUBSCRIBE, not the one on the wire.
#[wasm_bindgen]
pub struct PublishDoneMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(readonly, js_name = statusCode)]
    pub status_code: u64,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = errorReason)]
    pub error_reason: String,
}

impl PublishDoneMessage {
    pub(crate) fn new(request_id: u64, status_code: u64, error_reason: String) -> Self {
        Self {
            request_id,
            status_code,
            error_reason,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeNamespaceMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = trackNamespacePrefix)]
    pub track_namespace_prefix: Vec<String>,
}

impl From<&SubscribeNamespace> for SubscribeNamespaceMessage {
    fn from(message: &SubscribeNamespace) -> Self {
        Self {
            request_id: message.request_id,
            track_namespace_prefix: message.track_namespace_prefix.clone(),
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct NamespaceOkMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct RequestErrorMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(readonly, js_name = errorCode)]
    pub error_code: u64,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = reasonPhrase)]
    pub reason_phrase: String,
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct PublishMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = trackNamespace)]
    pub track_namespace: Vec<String>,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = trackName)]
    pub track_name: String,
    #[wasm_bindgen(readonly, js_name = trackAlias)]
    pub track_alias: u64,
    #[wasm_bindgen(readonly, js_name = groupOrder)]
    pub group_order: u8,
    #[wasm_bindgen(readonly, js_name = contentExists)]
    pub content_exists: bool,
    #[wasm_bindgen(readonly, js_name = largestGroupId)]
    pub largest_group_id: Option<u64>,
    #[wasm_bindgen(readonly, js_name = largestObjectId)]
    pub largest_object_id: Option<u64>,
    #[wasm_bindgen(readonly)]
    pub forward: bool,
}

impl From<&Publish> for PublishMessage {
    fn from(message: &Publish) -> Self {
        let (content_exists, largest_group_id, largest_object_id) =
            content_exists_fields(message.content_exists);
        Self {
            request_id: message.request_id,
            track_namespace: message.track_namespace_tuple.clone(),
            track_name: message.track_name.clone(),
            track_alias: message.track_alias,
            group_order: message.group_order as u8,
            content_exists,
            largest_group_id,
            largest_object_id,
            forward: message.forward,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct PublishOkMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(readonly)]
    pub forward: bool,
    #[wasm_bindgen(readonly, js_name = subscriberPriority)]
    pub subscriber_priority: u8,
    #[wasm_bindgen(readonly, js_name = groupOrder)]
    pub group_order: u8,
    #[wasm_bindgen(readonly, js_name = filterType)]
    pub filter_type: u8,
    #[wasm_bindgen(readonly, js_name = startGroup)]
    pub start_group: Option<u64>,
    #[wasm_bindgen(readonly, js_name = startObject)]
    pub start_object: Option<u64>,
    #[wasm_bindgen(readonly, js_name = endGroup)]
    pub end_group: Option<u64>,
    #[wasm_bindgen(readonly, js_name = deliveryTimeout)]
    pub delivery_timeout: Option<u64>,
}

impl From<&PublishOk> for PublishOkMessage {
    fn from(message: &PublishOk) -> Self {
        let (filter_type, start_group, start_object, end_group) =
            filter_fields(message.filter_type);
        Self {
            request_id: message.request_id,
            forward: message.forward,
            subscriber_priority: message.subscriber_priority,
            group_order: message.group_order as u8,
            filter_type,
            start_group,
            start_object,
            end_group,
            delivery_timeout: message.delivery_timeout,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = trackNamespace)]
    pub track_namespace: Vec<String>,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = trackName)]
    pub track_name: String,
    #[wasm_bindgen(readonly, js_name = subscriberPriority)]
    pub subscriber_priority: u8,
    #[wasm_bindgen(readonly, js_name = groupOrder)]
    pub group_order: u8,
    #[wasm_bindgen(readonly)]
    pub forward: bool,
    #[wasm_bindgen(readonly, js_name = filterType)]
    pub filter_type: u8,
    #[wasm_bindgen(readonly, js_name = startGroup)]
    pub start_group: Option<u64>,
    #[wasm_bindgen(readonly, js_name = startObject)]
    pub start_object: Option<u64>,
    #[wasm_bindgen(readonly, js_name = endGroup)]
    pub end_group: Option<u64>,
}

impl From<&Subscribe> for SubscribeMessage {
    fn from(message: &Subscribe) -> Self {
        let (filter_type, start_group, start_object, end_group) =
            filter_fields(message.filter_type);
        Self {
            request_id: message.request_id,
            track_namespace: message.track_namespace.clone(),
            track_name: message.track_name.clone(),
            subscriber_priority: message.subscriber_priority,
            group_order: message.group_order as u8,
            forward: message.forward,
            filter_type,
            start_group,
            start_object,
            end_group,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeOkMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(readonly, js_name = trackAlias)]
    pub track_alias: u64,
    #[wasm_bindgen(readonly)]
    pub expires: u64,
    #[wasm_bindgen(readonly, js_name = groupOrder)]
    pub group_order: u8,
    #[wasm_bindgen(readonly, js_name = contentExists)]
    pub content_exists: bool,
    #[wasm_bindgen(readonly, js_name = largestGroupId)]
    pub largest_group_id: Option<u64>,
    #[wasm_bindgen(readonly, js_name = largestObjectId)]
    pub largest_object_id: Option<u64>,
    #[wasm_bindgen(readonly, js_name = deliveryTimeout)]
    pub delivery_timeout: Option<u64>,
    #[wasm_bindgen(readonly, js_name = maxDuration)]
    pub max_duration: Option<u64>,
}

impl From<&SubscribeOk> for SubscribeOkMessage {
    fn from(message: &SubscribeOk) -> Self {
        let (content_exists, largest_group_id, largest_object_id) =
            content_exists_fields(message.content_exists);
        Self {
            request_id: message.request_id,
            track_alias: message.track_alias,
            expires: message.expires,
            group_order: message.group_order as u8,
            content_exists,
            largest_group_id,
            largest_object_id,
            delivery_timeout: message.delivery_timeout,
            max_duration: message.max_duration,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct SubgroupHeaderMessage {
    #[wasm_bindgen(readonly, js_name = trackAlias)]
    pub track_alias: u64,
    #[wasm_bindgen(readonly, js_name = groupId)]
    pub group_id: u64,
    #[wasm_bindgen(readonly, js_name = subgroupId)]
    pub subgroup_id: u64,
    #[wasm_bindgen(readonly, js_name = publisherPriority)]
    pub publisher_priority: u8,
}

impl SubgroupHeaderMessage {
    pub(crate) fn new(
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Self {
        Self {
            track_alias,
            group_id,
            subgroup_id,
            publisher_priority,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct ObjectDatagramMessage {
    #[wasm_bindgen(readonly, js_name = trackAlias)]
    pub track_alias: u64,
    #[wasm_bindgen(readonly, js_name = groupId)]
    pub group_id: u64,
    #[wasm_bindgen(readonly, js_name = objectId)]
    pub object_id: Option<u64>,
    #[wasm_bindgen(readonly, js_name = publisherPriority)]
    pub publisher_priority: u8,
    #[wasm_bindgen(readonly, js_name = objectPayloadLength)]
    pub object_payload_length: u32,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = objectPayload)]
    pub object_payload: Vec<u8>,
    loc_header: Vec<LocExtension>,
}

#[wasm_bindgen]
impl ObjectDatagramMessage {
    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        crate::loc::encode_loc_header(&self.loc_header)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

impl ObjectDatagramMessage {
    pub(crate) fn new(
        track_alias: u64,
        group_id: u64,
        object_id: Option<u64>,
        publisher_priority: u8,
        object_payload: Vec<u8>,
        loc_header: Vec<LocExtension>,
    ) -> Self {
        Self {
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            object_payload_length: object_payload.len() as u32,
            object_payload,
            loc_header,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct ObjectDatagramStatusMessage {
    #[wasm_bindgen(readonly, js_name = trackAlias)]
    pub track_alias: u64,
    #[wasm_bindgen(readonly, js_name = groupId)]
    pub group_id: u64,
    #[wasm_bindgen(readonly, js_name = objectId)]
    pub object_id: Option<u64>,
    #[wasm_bindgen(readonly, js_name = publisherPriority)]
    pub publisher_priority: u8,
    #[wasm_bindgen(readonly, js_name = objectStatus)]
    pub object_status: u8,
    loc_header: Vec<LocExtension>,
}

#[wasm_bindgen]
impl ObjectDatagramStatusMessage {
    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        crate::loc::encode_loc_header(&self.loc_header)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

impl ObjectDatagramStatusMessage {
    pub(crate) fn new(
        track_alias: u64,
        group_id: u64,
        object_id: Option<u64>,
        publisher_priority: u8,
        object_status: ObjectStatus,
        loc_header: Vec<LocExtension>,
    ) -> Self {
        Self {
            track_alias,
            group_id,
            object_id,
            publisher_priority,
            object_status: object_status as u8,
            loc_header,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct SubgroupObjectMessage {
    #[wasm_bindgen(readonly, js_name = subgroupId)]
    pub subgroup_id: u64,
    #[wasm_bindgen(readonly, js_name = objectId)]
    pub object_id: u64,
    #[wasm_bindgen(readonly, js_name = objectStatus)]
    pub object_status: Option<u8>,
    #[wasm_bindgen(readonly, js_name = objectPayloadLength)]
    pub object_payload_length: u32,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = objectPayload)]
    pub object_payload: Vec<u8>,
    loc_header: Vec<LocExtension>,
}

#[wasm_bindgen]
impl SubgroupObjectMessage {
    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        crate::loc::encode_loc_header(&self.loc_header)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

impl SubgroupObjectMessage {
    pub(crate) fn new(object: Object, loc_header: Vec<LocExtension>) -> Self {
        let ForwardingPreference::Subgroup { subgroup_id } = object.forwarding else {
            unreachable!("a subgroup stream carries subgroup objects")
        };
        let (object_status, object_payload) = if object.payload.is_empty() {
            (Some(object.status), Vec::new())
        } else {
            (None, object.payload.to_vec())
        };
        Self {
            subgroup_id,
            object_id: object.location.object_id,
            object_status: object_status.map(|value| value as u8),
            object_payload_length: object_payload.len() as u32,
            object_payload,
            loc_header,
        }
    }
}

/// The range is already resolved against the Largest Location this client
/// has sent: the end is the End Location its FETCH_OK carries (draft-14
/// §9.17), i.e. the Location after the last Object to send, where Object 0
/// covers the whole Group.
#[wasm_bindgen]
pub struct FetchMessage {
    #[wasm_bindgen(js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(getter_with_clone, js_name = trackNamespace)]
    pub track_namespace: Vec<String>,
    #[wasm_bindgen(getter_with_clone, js_name = trackName)]
    pub track_name: String,
    #[wasm_bindgen(js_name = groupOrder)]
    pub group_order: u8,
    #[wasm_bindgen(js_name = startGroupId)]
    pub start_group_id: u64,
    #[wasm_bindgen(js_name = startObjectId)]
    pub start_object_id: u64,
    #[wasm_bindgen(js_name = endGroupId)]
    pub end_group_id: u64,
    #[wasm_bindgen(js_name = endObjectId)]
    pub end_object_id: u64,
}

impl FetchMessage {
    pub(crate) fn new(request_id: u64, request: &IncomingFetchRequest) -> Self {
        Self {
            request_id,
            track_namespace: request.track_key.namespace.clone(),
            track_name: request.track_key.name.clone(),
            group_order: request.group_order as u8,
            start_group_id: request.start.group_id,
            start_object_id: request.start.object_id,
            end_group_id: request.end.group_id,
            end_object_id: request.end.object_id,
        }
    }
}

#[wasm_bindgen]
pub struct FetchOkMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(readonly, js_name = endGroupId)]
    pub end_group_id: u64,
    #[wasm_bindgen(readonly, js_name = endObjectId)]
    pub end_object_id: u64,
    #[wasm_bindgen(readonly, js_name = endOfTrack)]
    pub end_of_track: bool,
}

#[wasm_bindgen]
pub struct FetchObjectMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(readonly, js_name = groupId)]
    pub group_id: u64,
    #[wasm_bindgen(readonly, js_name = objectId)]
    pub object_id: u64,
    #[wasm_bindgen(getter_with_clone, readonly, js_name = objectPayload)]
    pub object_payload: Vec<u8>,
    loc_header: Vec<LocExtension>,
}

#[wasm_bindgen]
impl FetchObjectMessage {
    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        crate::loc::encode_loc_header(&self.loc_header)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

impl FetchObjectMessage {
    pub(crate) fn new(request_id: u64, object: &Object) -> Self {
        Self {
            request_id,
            group_id: object.location.group_id,
            object_id: object.location.object_id,
            object_payload: object.payload.to_vec(),
            loc_header: from_extension_headers(&object.extension_headers),
        }
    }
}

/// How the relay ended a fetch stream: a FIN means every object up to the
/// FETCH_OK End Location has been delivered; a RESET_STREAM means objects may
/// be missing.
#[wasm_bindgen]
pub struct FetchStreamEndMessage {
    #[wasm_bindgen(readonly, js_name = requestId)]
    pub request_id: u64,
    #[wasm_bindgen(readonly, js_name = resetErrorCode)]
    pub reset_error_code: Option<u64>,
}

#[wasm_bindgen]
impl FetchStreamEndMessage {
    #[wasm_bindgen(getter, js_name = isReset)]
    pub fn is_reset(&self) -> bool {
        self.reset_error_code.is_some()
    }
}

impl FetchStreamEndMessage {
    pub(crate) fn finished(request_id: u64) -> Self {
        Self {
            request_id,
            reset_error_code: None,
        }
    }

    pub(crate) fn reset(request_id: u64, error_code: u64) -> Self {
        Self {
            request_id,
            reset_error_code: Some(error_code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use moqt::object::{ExtensionHeaders, Location};

    fn subgroup_object(object_id: u64, status: ObjectStatus, payload: &'static [u8]) -> Object {
        Object {
            location: Location {
                group_id: 2,
                object_id,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 0,
            status,
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::from_static(payload),
            ends_group: false,
        }
    }

    #[test]
    fn reports_the_absolute_object_id_and_its_payload() {
        // Arrange
        let object = subgroup_object(7, ObjectStatus::Normal, b"ab");

        // Act
        let message = SubgroupObjectMessage::new(object, Vec::new());

        // Assert
        assert_eq!(message.object_id, 7);
        assert_eq!(message.object_status, None);
        assert_eq!(message.object_payload, b"ab".to_vec());
        assert_eq!(message.object_payload_length, 2);
    }

    #[test]
    fn carries_a_status_object_without_payload() {
        // Arrange
        let object = subgroup_object(30, ObjectStatus::EndOfGroup, b"");

        // Act
        let message = SubgroupObjectMessage::new(object, Vec::new());

        // Assert
        assert_eq!(message.object_status, Some(ObjectStatus::EndOfGroup as u8));
        assert_eq!(message.object_payload_length, 0);
    }
}

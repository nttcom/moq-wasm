mod subgroup_state;

use moqt::wire::{
    ContentExists, FilterType, NamespaceOk, ObjectStatus, Publish, PublishNamespace, PublishOk,
    RequestError, ServerSetup, Subscribe, SubscribeNamespace, SubscribeOk,
};
use packages::loc::LocHeader;
pub use subgroup_state::SubgroupState;
use wasm_bindgen::prelude::*;

fn filter_fields(filter_type: FilterType) -> (u8, Option<u64>, Option<u64>, Option<u64>) {
    match filter_type {
        FilterType::LatestGroup => (1, None, None, None),
        FilterType::LatestObject => (2, None, None, None),
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
    version: u32,
    max_request_id: u64,
}

#[wasm_bindgen]
impl ServerSetupMessage {
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> u32 {
        self.version
    }

    #[wasm_bindgen(getter, js_name = maxRequestId)]
    pub fn max_request_id(&self) -> u64 {
        self.max_request_id
    }
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
    request_id: u64,
    track_namespace: Vec<String>,
}

#[wasm_bindgen]
impl PublishNamespaceMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    #[wasm_bindgen(getter, js_name = trackNamespace)]
    pub fn track_namespace(&self) -> Vec<String> {
        self.track_namespace.clone()
    }
}

impl From<&PublishNamespace> for PublishNamespaceMessage {
    fn from(message: &PublishNamespace) -> Self {
        Self {
            request_id: message.request_id,
            track_namespace: message.track_namespace.clone(),
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct SubscribeNamespaceMessage {
    request_id: u64,
    track_namespace_prefix: Vec<String>,
}

#[wasm_bindgen]
impl SubscribeNamespaceMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    #[wasm_bindgen(getter, js_name = trackNamespacePrefix)]
    pub fn track_namespace_prefix(&self) -> Vec<String> {
        self.track_namespace_prefix.clone()
    }
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
    request_id: u64,
}

#[wasm_bindgen]
impl NamespaceOkMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }
}

impl From<&NamespaceOk> for NamespaceOkMessage {
    fn from(message: &NamespaceOk) -> Self {
        Self {
            request_id: message.request_id,
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct RequestErrorMessage {
    request_id: u64,
    error_code: u64,
    reason_phrase: String,
}

#[wasm_bindgen]
impl RequestErrorMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
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

impl From<&RequestError> for RequestErrorMessage {
    fn from(message: &RequestError) -> Self {
        Self {
            request_id: message.request_id,
            error_code: message.error_code,
            reason_phrase: message.reason_phrase.clone(),
        }
    }
}

#[wasm_bindgen]
#[derive(Clone)]
pub struct PublishMessage {
    request_id: u64,
    track_namespace: Vec<String>,
    track_name: String,
    track_alias: u64,
    group_order: u8,
    content_exists: bool,
    largest_group_id: Option<u64>,
    largest_object_id: Option<u64>,
    forward: bool,
}

#[wasm_bindgen]
impl PublishMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    #[wasm_bindgen(getter, js_name = trackNamespace)]
    pub fn track_namespace(&self) -> Vec<String> {
        self.track_namespace.clone()
    }

    #[wasm_bindgen(getter, js_name = trackName)]
    pub fn track_name(&self) -> String {
        self.track_name.clone()
    }

    #[wasm_bindgen(getter, js_name = trackAlias)]
    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    #[wasm_bindgen(getter, js_name = groupOrder)]
    pub fn group_order(&self) -> u8 {
        self.group_order
    }

    #[wasm_bindgen(getter, js_name = contentExists)]
    pub fn content_exists(&self) -> bool {
        self.content_exists
    }

    #[wasm_bindgen(getter, js_name = largestGroupId)]
    pub fn largest_group_id(&self) -> Option<u64> {
        self.largest_group_id
    }

    #[wasm_bindgen(getter, js_name = largestObjectId)]
    pub fn largest_object_id(&self) -> Option<u64> {
        self.largest_object_id
    }

    #[wasm_bindgen(getter)]
    pub fn forward(&self) -> bool {
        self.forward
    }
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
    request_id: u64,
    forward: bool,
    subscriber_priority: u8,
    group_order: u8,
    filter_type: u8,
    start_group: Option<u64>,
    start_object: Option<u64>,
    end_group: Option<u64>,
    delivery_timeout: Option<u64>,
}

#[wasm_bindgen]
impl PublishOkMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    #[wasm_bindgen(getter)]
    pub fn forward(&self) -> bool {
        self.forward
    }

    #[wasm_bindgen(getter, js_name = subscriberPriority)]
    pub fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    #[wasm_bindgen(getter, js_name = groupOrder)]
    pub fn group_order(&self) -> u8 {
        self.group_order
    }

    #[wasm_bindgen(getter, js_name = filterType)]
    pub fn filter_type(&self) -> u8 {
        self.filter_type
    }

    #[wasm_bindgen(getter, js_name = startGroup)]
    pub fn start_group(&self) -> Option<u64> {
        self.start_group
    }

    #[wasm_bindgen(getter, js_name = startObject)]
    pub fn start_object(&self) -> Option<u64> {
        self.start_object
    }

    #[wasm_bindgen(getter, js_name = endGroup)]
    pub fn end_group(&self) -> Option<u64> {
        self.end_group
    }

    #[wasm_bindgen(getter, js_name = deliveryTimeout)]
    pub fn delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }
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
    request_id: u64,
    track_namespace: Vec<String>,
    track_name: String,
    subscriber_priority: u8,
    group_order: u8,
    forward: bool,
    filter_type: u8,
    start_group: Option<u64>,
    start_object: Option<u64>,
    end_group: Option<u64>,
}

#[wasm_bindgen]
impl SubscribeMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    #[wasm_bindgen(getter, js_name = trackNamespace)]
    pub fn track_namespace(&self) -> Vec<String> {
        self.track_namespace.clone()
    }

    #[wasm_bindgen(getter, js_name = trackName)]
    pub fn track_name(&self) -> String {
        self.track_name.clone()
    }

    #[wasm_bindgen(getter, js_name = subscriberPriority)]
    pub fn subscriber_priority(&self) -> u8 {
        self.subscriber_priority
    }

    #[wasm_bindgen(getter, js_name = groupOrder)]
    pub fn group_order(&self) -> u8 {
        self.group_order
    }

    #[wasm_bindgen(getter)]
    pub fn forward(&self) -> bool {
        self.forward
    }

    #[wasm_bindgen(getter, js_name = filterType)]
    pub fn filter_type(&self) -> u8 {
        self.filter_type
    }

    #[wasm_bindgen(getter, js_name = startGroup)]
    pub fn start_group(&self) -> Option<u64> {
        self.start_group
    }

    #[wasm_bindgen(getter, js_name = startObject)]
    pub fn start_object(&self) -> Option<u64> {
        self.start_object
    }

    #[wasm_bindgen(getter, js_name = endGroup)]
    pub fn end_group(&self) -> Option<u64> {
        self.end_group
    }
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
    request_id: u64,
    track_alias: u64,
    expires: u64,
    group_order: u8,
    content_exists: bool,
    largest_group_id: Option<u64>,
    largest_object_id: Option<u64>,
    delivery_timeout: Option<u64>,
    max_duration: Option<u64>,
}

#[wasm_bindgen]
impl SubscribeOkMessage {
    #[wasm_bindgen(getter, js_name = requestId)]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    #[wasm_bindgen(getter, js_name = trackAlias)]
    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    #[wasm_bindgen(getter)]
    pub fn expires(&self) -> u64 {
        self.expires
    }

    #[wasm_bindgen(getter, js_name = groupOrder)]
    pub fn group_order(&self) -> u8 {
        self.group_order
    }

    #[wasm_bindgen(getter, js_name = contentExists)]
    pub fn content_exists(&self) -> bool {
        self.content_exists
    }

    #[wasm_bindgen(getter, js_name = largestGroupId)]
    pub fn largest_group_id(&self) -> Option<u64> {
        self.largest_group_id
    }

    #[wasm_bindgen(getter, js_name = largestObjectId)]
    pub fn largest_object_id(&self) -> Option<u64> {
        self.largest_object_id
    }

    #[wasm_bindgen(getter, js_name = deliveryTimeout)]
    pub fn delivery_timeout(&self) -> Option<u64> {
        self.delivery_timeout
    }

    #[wasm_bindgen(getter, js_name = maxDuration)]
    pub fn max_duration(&self) -> Option<u64> {
        self.max_duration
    }
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
    track_alias: u64,
    group_id: u64,
    subgroup_id: Option<u64>,
    publisher_priority: u8,
}

#[wasm_bindgen]
impl SubgroupHeaderMessage {
    #[wasm_bindgen(getter, js_name = trackAlias)]
    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    #[wasm_bindgen(getter, js_name = subgroupId)]
    pub fn subgroup_id(&self) -> Option<u64> {
        self.subgroup_id
    }

    #[wasm_bindgen(getter, js_name = publisherPriority)]
    pub fn publisher_priority(&self) -> u8 {
        self.publisher_priority
    }
}

impl SubgroupHeaderMessage {
    pub(crate) fn new(
        track_alias: u64,
        group_id: u64,
        subgroup_id: Option<u64>,
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
    track_alias: u64,
    group_id: u64,
    object_id: Option<u64>,
    publisher_priority: u8,
    object_payload_length: u32,
    object_payload: Vec<u8>,
    loc_header: LocHeader,
}

#[wasm_bindgen]
impl ObjectDatagramMessage {
    #[wasm_bindgen(getter, js_name = trackAlias)]
    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    #[wasm_bindgen(getter, js_name = objectId)]
    pub fn object_id(&self) -> Option<u64> {
        self.object_id
    }

    #[wasm_bindgen(getter, js_name = publisherPriority)]
    pub fn publisher_priority(&self) -> u8 {
        self.publisher_priority
    }

    #[wasm_bindgen(getter, js_name = objectPayloadLength)]
    pub fn object_payload_length(&self) -> u32 {
        self.object_payload_length
    }

    #[wasm_bindgen(getter, js_name = objectPayload)]
    pub fn object_payload(&self) -> Vec<u8> {
        self.object_payload.clone()
    }

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
        loc_header: LocHeader,
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
    track_alias: u64,
    group_id: u64,
    object_id: Option<u64>,
    publisher_priority: u8,
    object_status: u8,
    loc_header: LocHeader,
}

#[wasm_bindgen]
impl ObjectDatagramStatusMessage {
    #[wasm_bindgen(getter, js_name = trackAlias)]
    pub fn track_alias(&self) -> u64 {
        self.track_alias
    }

    #[wasm_bindgen(getter, js_name = groupId)]
    pub fn group_id(&self) -> u64 {
        self.group_id
    }

    #[wasm_bindgen(getter, js_name = objectId)]
    pub fn object_id(&self) -> Option<u64> {
        self.object_id
    }

    #[wasm_bindgen(getter, js_name = publisherPriority)]
    pub fn publisher_priority(&self) -> u8 {
        self.publisher_priority
    }

    #[wasm_bindgen(getter, js_name = objectStatus)]
    pub fn object_status(&self) -> u8 {
        self.object_status
    }

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
        loc_header: LocHeader,
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
    subgroup_id: Option<u64>,
    object_id_delta: u64,
    object_status: Option<u8>,
    object_payload_length: u32,
    object_payload: Vec<u8>,
    loc_header: LocHeader,
}

#[wasm_bindgen]
impl SubgroupObjectMessage {
    #[wasm_bindgen(getter, js_name = subgroupId)]
    pub fn subgroup_id(&self) -> Option<u64> {
        self.subgroup_id
    }

    #[wasm_bindgen(getter, js_name = objectIdDelta)]
    pub fn object_id_delta(&self) -> u64 {
        self.object_id_delta
    }

    #[wasm_bindgen(getter, js_name = objectId)]
    pub fn object_id(&self) -> u64 {
        self.object_id_delta
    }

    #[wasm_bindgen(getter, js_name = objectStatus)]
    pub fn object_status(&self) -> Option<u8> {
        self.object_status
    }

    #[wasm_bindgen(getter, js_name = objectPayloadLength)]
    pub fn object_payload_length(&self) -> u32 {
        self.object_payload_length
    }

    #[wasm_bindgen(getter, js_name = objectPayload)]
    pub fn object_payload(&self) -> Vec<u8> {
        self.object_payload.clone()
    }

    #[wasm_bindgen(getter, js_name = locHeader)]
    pub fn loc_header(&self) -> Result<JsValue, JsValue> {
        crate::loc::encode_loc_header(&self.loc_header)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    }
}

impl SubgroupObjectMessage {
    pub(crate) fn new(
        subgroup_id: Option<u64>,
        object_id_delta: u64,
        object_status: Option<ObjectStatus>,
        object_payload: Vec<u8>,
        loc_header: LocHeader,
    ) -> Self {
        Self {
            subgroup_id,
            object_id_delta,
            object_status: object_status.map(|value| value as u8),
            object_payload_length: object_payload.len() as u32,
            object_payload,
            loc_header,
        }
    }
}

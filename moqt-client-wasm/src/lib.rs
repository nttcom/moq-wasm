mod utils;

#[cfg(feature = "web_sys_unstable_apis")]
mod loc;
mod media_streaming_format;
#[cfg(feature = "web_sys_unstable_apis")]
mod messages;

pub use media_streaming_format::*;
#[cfg(feature = "web_sys_unstable_apis")]
pub use messages::*;

#[cfg(feature = "web_sys_unstable_apis")]
use anyhow::{Result, anyhow};
#[cfg(feature = "web_sys_unstable_apis")]
use bytes::{Buf, Bytes, BytesMut};
#[cfg(feature = "web_sys_unstable_apis")]
use moqt::wire::{
    AuthorizationToken, BufGetExt, BufPutExt, ClientSetup, ContentExists, ControlMessageType,
    DatagramField, ExtensionHeaders, FilterType, GroupOrder, Location, NamespaceOk, ObjectDatagram,
    ObjectStatus, Publish, PublishNamespace, PublishOk, RequestError, ServerSetup, SetupParameter,
    SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField, Subscribe, SubscribeNamespace,
    SubscribeOk, encode_control_message, take_control_message,
};
#[cfg(feature = "web_sys_unstable_apis")]
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    io::Cursor,
    rc::Rc,
};
#[cfg(feature = "web_sys_unstable_apis")]
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
#[cfg(feature = "web_sys_unstable_apis")]
use wasm_bindgen_futures::JsFuture;
#[cfg(feature = "web_sys_unstable_apis")]
use web_sys::{
    ReadableStream, ReadableStreamDefaultReader, WebTransport, WebTransportBidirectionalStream,
    WritableStreamDefaultWriter,
};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[cfg(feature = "web_sys_unstable_apis")]
macro_rules! console_log {
    ($($t:tt)*) => {
        log(&format_args!($($t)*).to_string())
    };
}

#[wasm_bindgen(start)]
fn main() {
    utils::set_panic_hook();
}

#[cfg(feature = "web_sys_unstable_apis")]
type WriterKey = (u64, u64, u64);

#[cfg(feature = "web_sys_unstable_apis")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TrackKey {
    namespace: Vec<String>,
    name: String,
}

#[cfg(feature = "web_sys_unstable_apis")]
impl TrackKey {
    fn new(namespace: Vec<String>, name: String) -> Self {
        Self { namespace, name }
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
#[derive(Debug, Clone)]
struct OutgoingSubscribeRequest {
    track_key: TrackKey,
    track_alias: Option<u64>,
}

#[cfg(feature = "web_sys_unstable_apis")]
#[derive(Debug, Clone)]
struct IncomingSubscribeRequest {
    track_key: TrackKey,
    group_order: GroupOrder,
    track_alias: Option<u64>,
}

#[cfg(feature = "web_sys_unstable_apis")]
#[derive(Debug, Default)]
struct ClientState {
    max_request_id: u64,
    published_namespaces: HashSet<Vec<String>>,
    subscribed_namespace_prefixes: HashSet<Vec<String>>,
    publish_namespace_requests: HashMap<u64, Vec<String>>,
    subscribe_namespace_requests: HashMap<u64, Vec<String>>,
    publish_requests: HashMap<u64, TrackKey>,
    outgoing_subscriptions: HashMap<u64, OutgoingSubscribeRequest>,
    incoming_subscriptions: HashMap<u64, IncomingSubscribeRequest>,
    publishing_track_aliases: HashMap<TrackKey, BTreeSet<u64>>,
    alias_to_track_key: HashMap<u64, TrackKey>,
    subgroup_states: HashMap<u64, SubgroupState>,
    next_track_alias: u64,
}

#[cfg(feature = "web_sys_unstable_apis")]
impl ClientState {
    fn configure(&mut self, max_request_id: u64) {
        self.max_request_id = max_request_id;
    }

    fn contains_published_namespace(&self, namespace: &[String]) -> bool {
        self.published_namespaces.contains(namespace)
    }

    fn register_publish_namespace_request(&mut self, request_id: u64, namespace: Vec<String>) {
        self.published_namespaces.insert(namespace.clone());
        self.publish_namespace_requests
            .insert(request_id, namespace);
    }

    fn finish_publish_namespace_request(&mut self, request_id: u64, success: bool) {
        if let Some(namespace) = self.publish_namespace_requests.remove(&request_id)
            && !success
        {
            self.published_namespaces.remove(&namespace);
        }
    }

    fn register_subscribe_namespace_request(
        &mut self,
        request_id: u64,
        namespace_prefix: Vec<String>,
    ) {
        self.subscribed_namespace_prefixes
            .insert(namespace_prefix.clone());
        self.subscribe_namespace_requests
            .insert(request_id, namespace_prefix);
    }

    fn finish_subscribe_namespace_request(&mut self, request_id: u64, success: bool) {
        if let Some(namespace_prefix) = self.subscribe_namespace_requests.remove(&request_id)
            && !success
        {
            self.subscribed_namespace_prefixes.remove(&namespace_prefix);
        }
    }

    fn register_publish_request(&mut self, request_id: u64, track_key: TrackKey) {
        self.publish_requests.insert(request_id, track_key);
    }

    fn finish_publish_request(&mut self, request_id: u64) {
        self.publish_requests.remove(&request_id);
    }

    fn start_outgoing_subscription(&mut self, request_id: u64, track_key: TrackKey) {
        self.outgoing_subscriptions.insert(
            request_id,
            OutgoingSubscribeRequest {
                track_key,
                track_alias: None,
            },
        );
    }

    fn activate_outgoing_subscription(&mut self, request_id: u64, track_alias: u64) {
        if let Some(subscription) = self.outgoing_subscriptions.get_mut(&request_id) {
            subscription.track_alias = Some(track_alias);
            self.alias_to_track_key
                .insert(track_alias, subscription.track_key.clone());
        }
    }

    fn remove_outgoing_subscription(&mut self, request_id: u64) -> Option<u64> {
        let track_alias = self
            .outgoing_subscriptions
            .remove(&request_id)
            .and_then(|subscription| subscription.track_alias);
        if let Some(track_alias) = track_alias {
            self.alias_to_track_key.remove(&track_alias);
            self.subgroup_states.remove(&track_alias);
        }
        track_alias
    }

    fn validate_incoming_subscribe(&self, message: &Subscribe) -> u64 {
        if !self.contains_published_namespace(&message.track_namespace) {
            return 404;
        }
        if self
            .incoming_subscriptions
            .contains_key(&message.request_id)
        {
            return 409;
        }
        0
    }

    fn register_incoming_subscribe(&mut self, message: &Subscribe) {
        self.incoming_subscriptions.insert(
            message.request_id,
            IncomingSubscribeRequest {
                track_key: TrackKey::new(
                    message.track_namespace.clone(),
                    message.track_name.clone(),
                ),
                group_order: message.group_order,
                track_alias: None,
            },
        );
    }

    fn allocate_track_alias(&mut self) -> u64 {
        let track_alias = self.next_track_alias;
        self.next_track_alias = self.next_track_alias.saturating_add(1);
        track_alias
    }

    fn activate_incoming_subscribe(&mut self, request_id: u64) -> Result<u64> {
        let new_track_alias = self.allocate_track_alias();
        let (track_alias, track_key) = {
            let entry = self
                .incoming_subscriptions
                .get_mut(&request_id)
                .ok_or_else(|| anyhow!("unknown subscribe request: {request_id}"))?;
            let track_alias = entry.track_alias.unwrap_or(new_track_alias);
            entry.track_alias = Some(track_alias);
            (track_alias, entry.track_key.clone())
        };

        self.alias_to_track_key
            .insert(track_alias, track_key.clone());
        self.publishing_track_aliases
            .entry(track_key)
            .or_default()
            .insert(track_alias);

        Ok(track_alias)
    }

    fn incoming_subscribe_group_order(&self, request_id: u64) -> Result<GroupOrder> {
        self.incoming_subscriptions
            .get(&request_id)
            .map(|entry| entry.group_order)
            .ok_or_else(|| anyhow!("unknown subscribe request: {request_id}"))
    }

    fn remove_incoming_subscribe(&mut self, request_id: u64) -> Option<u64> {
        let removed = self.incoming_subscriptions.remove(&request_id)?;
        if let Some(track_alias) = removed.track_alias {
            self.alias_to_track_key.remove(&track_alias);
            self.subgroup_states.remove(&track_alias);
            if let Some(aliases) = self.publishing_track_aliases.get_mut(&removed.track_key) {
                aliases.remove(&track_alias);
                if aliases.is_empty() {
                    self.publishing_track_aliases.remove(&removed.track_key);
                }
            }
            return Some(track_alias);
        }
        None
    }

    fn get_track_subscribers(&self, namespace: Vec<String>, track_name: String) -> Vec<u64> {
        self.publishing_track_aliases
            .get(&TrackKey::new(namespace, track_name))
            .map(|aliases| aliases.iter().copied().collect())
            .unwrap_or_default()
    }

    fn subgroup_state_entry(&mut self, track_alias: u64) -> &mut SubgroupState {
        self.subgroup_states
            .entry(track_alias)
            .or_insert_with(|| SubgroupState::with_track(track_alias))
    }

    fn current_subgroup_state(&mut self, track_alias: u64) -> SubgroupState {
        self.subgroup_state_entry(track_alias).clone()
    }

    fn mark_subgroup_header_sent(&mut self, track_alias: u64) {
        self.subgroup_state_entry(track_alias).mark_header_sent();
    }

    fn increment_subgroup_object(&mut self, track_alias: u64) {
        self.subgroup_state_entry(track_alias).increment_object_id();
    }

    fn reset_subgroup_state(&mut self, track_alias: u64) {
        self.subgroup_states.remove(&track_alias);
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
#[wasm_bindgen]
pub struct MOQTClient {
    url: String,
    state: Rc<RefCell<ClientState>>,
    transport: Rc<RefCell<Option<WebTransport>>>,
    control_stream_writer: Rc<RefCell<Option<WritableStreamDefaultWriter>>>,
    datagram_writer: Rc<RefCell<Option<WritableStreamDefaultWriter>>>,
    stream_writers: Rc<RefCell<HashMap<WriterKey, WritableStreamDefaultWriter>>>,
    stream_object_numbers: Rc<RefCell<HashMap<WriterKey, u64>>>,
    callbacks: Rc<RefCell<MOQTCallbacks>>,
}

#[cfg(feature = "web_sys_unstable_apis")]
#[wasm_bindgen]
impl MOQTClient {
    #[wasm_bindgen(constructor)]
    pub fn new(url: String) -> Self {
        Self {
            url,
            state: Rc::new(RefCell::new(ClientState::default())),
            transport: Rc::new(RefCell::new(None)),
            control_stream_writer: Rc::new(RefCell::new(None)),
            datagram_writer: Rc::new(RefCell::new(None)),
            stream_writers: Rc::new(RefCell::new(HashMap::new())),
            stream_object_numbers: Rc::new(RefCell::new(HashMap::new())),
            callbacks: Rc::new(RefCell::new(MOQTCallbacks::default())),
        }
    }

    pub fn url(&self) -> JsValue {
        JsValue::from_str(&self.url)
    }

    #[wasm_bindgen(js_name = onServerSetup)]
    pub fn set_server_setup_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().server_setup_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublishNamespace)]
    pub fn set_publish_namespace_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().publish_namespace_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublishNamespaceResponse)]
    pub fn set_publish_namespace_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .publish_namespace_response_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubscribeNamespaceResponse)]
    pub fn set_subscribe_namespace_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .subscribe_namespace_response_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublish)]
    pub fn set_publish_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().publish_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onPublishResponse)]
    pub fn set_publish_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().publish_response_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubscribe)]
    pub fn set_subscribe_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().subscribe_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubscribeResponse)]
    pub fn set_subscribe_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().subscribe_response_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onIncomingUnsubscribe)]
    pub fn set_incoming_unsubscribe_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().incoming_unsubscribe_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onObjectDatagram)]
    pub fn set_object_datagram_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().object_datagram_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onObjectDatagramStatus)]
    pub fn set_object_datagram_status_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().object_datagram_status_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubgroupHeader)]
    pub fn set_subgroup_header_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().subgroup_header_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onSubgroupObject)]
    pub fn set_subgroup_object_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().subgroup_object_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = onConnectionClosed)]
    pub fn set_connection_closed_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().connection_closed_callback = Some(callback);
    }

    #[wasm_bindgen(js_name = isConnected)]
    pub fn is_connected(&self) -> bool {
        self.transport.borrow().is_some()
    }

    #[wasm_bindgen(js_name = sendClientSetup)]
    pub async fn send_client_setup(
        &self,
        versions: Vec<u64>,
        max_request_id: u64,
    ) -> Result<(), JsValue> {
        let supported_versions = versions.into_iter().map(|version| version as u32).collect();
        let payload =
            ClientSetup::new(supported_versions, default_setup_parameters(max_request_id)).encode();
        self.state.borrow_mut().configure(max_request_id);
        self.send_control_message(ControlMessageType::ClientSetup, payload)
            .await
    }

    #[wasm_bindgen(js_name = sendPublishNamespace)]
    pub async fn send_publish_namespace(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        auth_info: String,
    ) -> Result<(), JsValue> {
        if self
            .state
            .borrow()
            .contains_published_namespace(&track_namespace)
        {
            return Ok(());
        }

        let payload = PublishNamespace::new(
            request_id,
            track_namespace.clone(),
            authorization_tokens(&auth_info),
        )
        .encode();
        self.state
            .borrow_mut()
            .register_publish_namespace_request(request_id, track_namespace);
        self.send_control_message(ControlMessageType::PublishNamespace, payload)
            .await
    }

    #[wasm_bindgen(js_name = sendPublishNamespaceOk)]
    pub async fn send_publish_namespace_ok(&self, request_id: u64) -> Result<(), JsValue> {
        self.send_control_message(
            ControlMessageType::PublishNamespaceOk,
            NamespaceOk { request_id }.encode(),
        )
        .await
    }

    #[wasm_bindgen(js_name = sendPublishNamespaceError)]
    pub async fn send_publish_namespace_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        self.send_request_error(
            ControlMessageType::PublishNamespaceError,
            request_id,
            error_code,
            reason_phrase,
        )
        .await
    }

    #[wasm_bindgen(js_name = sendSubscribeNamespace)]
    pub async fn send_subscribe_namespace(
        &self,
        request_id: u64,
        track_namespace_prefix: Vec<String>,
        auth_info: String,
    ) -> Result<(), JsValue> {
        if self
            .state
            .borrow()
            .subscribed_namespace_prefixes
            .contains(&track_namespace_prefix)
        {
            return Ok(());
        }

        let payload = SubscribeNamespace::new(
            request_id,
            track_namespace_prefix.clone(),
            authorization_tokens(&auth_info),
        )
        .encode();
        self.state
            .borrow_mut()
            .register_subscribe_namespace_request(request_id, track_namespace_prefix);
        self.send_control_message(ControlMessageType::SubscribeNamespace, payload)
            .await
    }

    #[wasm_bindgen(js_name = sendPublish)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_publish(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
        track_alias: u64,
        group_order: u8,
        content_exists: bool,
        largest_group_id: Option<u64>,
        largest_object_id: Option<u64>,
        forward: bool,
        auth_info: String,
    ) -> Result<(), JsValue> {
        let group_order =
            GroupOrder::try_from(group_order).map_err(|_| js_error("invalid group order"))?;
        let content_exists =
            content_exists_from_fields(content_exists, largest_group_id, largest_object_id);
        let payload = Publish {
            request_id,
            track_namespace_tuple: track_namespace.clone(),
            track_name: track_name.clone(),
            track_alias,
            group_order,
            content_exists,
            forward,
            authorization_tokens: authorization_tokens(&auth_info),
            delivery_timeout: None,
            max_duration: None,
        }
        .encode();
        self.state
            .borrow_mut()
            .register_publish_request(request_id, TrackKey::new(track_namespace, track_name));
        self.send_control_message(ControlMessageType::Publish, payload)
            .await
    }

    #[wasm_bindgen(js_name = sendPublishOk)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_publish_ok(
        &self,
        request_id: u64,
        subscriber_priority: u8,
        group_order: u8,
        filter_type: u8,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        delivery_timeout: Option<u64>,
        forward: bool,
    ) -> Result<(), JsValue> {
        let group_order =
            GroupOrder::try_from(group_order).map_err(|_| js_error("invalid group order"))?;
        let filter_type =
            filter_type_from_fields(filter_type, start_group, start_object, end_group)?;
        let payload = PublishOk {
            request_id,
            forward,
            subscriber_priority,
            group_order,
            filter_type,
            delivery_timeout,
        }
        .encode();
        self.send_control_message(ControlMessageType::PublishOk, payload)
            .await
    }

    #[wasm_bindgen(js_name = sendPublishError)]
    pub async fn send_publish_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        self.send_request_error(
            ControlMessageType::PublishError,
            request_id,
            error_code,
            reason_phrase,
        )
        .await
    }

    #[wasm_bindgen(js_name = sendSubscribe)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_subscribe(
        &self,
        request_id: u64,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: u8,
        filter_type: u8,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
        auth_info: String,
        forward: bool,
        delivery_timeout: Option<u64>,
    ) -> Result<(), JsValue> {
        let group_order =
            GroupOrder::try_from(group_order).map_err(|_| js_error("invalid group order"))?;
        let filter_type =
            filter_type_from_fields(filter_type, start_group, start_object, end_group)?;
        let payload = Subscribe {
            request_id,
            track_namespace: track_namespace.clone(),
            track_name: track_name.clone(),
            subscriber_priority,
            group_order,
            forward,
            filter_type,
            authorization_tokens: authorization_tokens(&auth_info),
            delivery_timeout,
        }
        .encode();
        self.state
            .borrow_mut()
            .start_outgoing_subscription(request_id, TrackKey::new(track_namespace, track_name));
        self.send_control_message(ControlMessageType::Subscribe, payload)
            .await
    }

    #[wasm_bindgen(js_name = isSubscribed)]
    pub fn is_subscribed(&self, request_id: u64) -> bool {
        self.state
            .borrow()
            .outgoing_subscriptions
            .contains_key(&request_id)
    }

    #[wasm_bindgen(js_name = getTrackSubscribers)]
    pub fn get_track_subscribers(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Vec<u64> {
        self.state
            .borrow()
            .get_track_subscribers(track_namespace, track_name)
    }

    #[wasm_bindgen(js_name = getSubgroupState)]
    pub fn get_subgroup_state(&self, track_alias: u64) -> SubgroupState {
        self.state.borrow_mut().current_subgroup_state(track_alias)
    }

    #[wasm_bindgen(js_name = markSubgroupHeaderSent)]
    pub fn mark_subgroup_header_sent(&self, track_alias: u64) {
        self.state
            .borrow_mut()
            .mark_subgroup_header_sent(track_alias);
    }

    #[wasm_bindgen(js_name = incrementSubgroupObject)]
    pub fn increment_subgroup_object(&self, track_alias: u64) {
        self.state
            .borrow_mut()
            .increment_subgroup_object(track_alias);
    }

    #[wasm_bindgen(js_name = resetSubgroupState)]
    pub fn reset_subgroup_state(&self, track_alias: u64) {
        self.state.borrow_mut().reset_subgroup_state(track_alias);
    }

    #[wasm_bindgen(js_name = sendSubscribeOk)]
    pub async fn send_subscribe_ok(
        &self,
        request_id: u64,
        expires: u64,
        content_exists: bool,
        largest_group_id: Option<u64>,
        largest_object_id: Option<u64>,
        delivery_timeout: Option<u64>,
        max_duration: Option<u64>,
    ) -> Result<u64, JsValue> {
        let (track_alias, group_order) = {
            let mut state = self.state.borrow_mut();
            let group_order = state
                .incoming_subscribe_group_order(request_id)
                .map_err(|error| js_error(error.to_string()))?;
            let group_order = if group_order == GroupOrder::Publisher {
                GroupOrder::Ascending
            } else {
                group_order
            };
            let track_alias = state
                .activate_incoming_subscribe(request_id)
                .map_err(|error| js_error(error.to_string()))?;
            (track_alias, group_order)
        };

        let payload = SubscribeOk {
            request_id,
            track_alias,
            expires,
            group_order,
            content_exists: content_exists_from_fields(
                content_exists,
                largest_group_id,
                largest_object_id,
            ),
            delivery_timeout,
            max_duration,
        }
        .encode();
        self.send_control_message(ControlMessageType::SubscribeOk, payload)
            .await?;
        Ok(track_alias)
    }

    #[wasm_bindgen(js_name = sendSubscribeError)]
    pub async fn send_subscribe_error(
        &self,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        self.send_request_error(
            ControlMessageType::SubscribeError,
            request_id,
            error_code,
            reason_phrase,
        )
        .await
    }

    #[wasm_bindgen(js_name = sendUnsubscribe)]
    pub async fn send_unsubscribe(&self, request_id: u64) -> Result<(), JsValue> {
        let mut payload = BytesMut::new();
        payload.put_varint(request_id);
        self.send_control_message(ControlMessageType::UnSubscribe, payload)
            .await?;
        self.state
            .borrow_mut()
            .remove_outgoing_subscription(request_id);
        Ok(())
    }

    #[wasm_bindgen(js_name = sendObjectDatagram)]
    pub async fn send_object_datagram(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_payload: Vec<u8>,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let extension_headers = match crate::loc::parse_loc_header(loc_header) {
            Ok(Some(header)) => crate::loc::loc_header_to_extension_headers(&header),
            Ok(None) => Ok(empty_extension_headers()),
            Err(error) => Err(error),
        }
        .map_err(|error| js_error(error.to_string()))?;

        let field = if extension_headers == empty_extension_headers() {
            DatagramField::Payload0x00 {
                object_id,
                publisher_priority,
                payload: Bytes::from(object_payload),
            }
        } else {
            DatagramField::Payload0x01 {
                object_id,
                publisher_priority,
                extension_headers,
                payload: Bytes::from(object_payload),
            }
        };

        let payload = ObjectDatagram::new(track_alias, group_id, field).encode();
        self.send_datagram_bytes(&payload).await
    }

    #[wasm_bindgen(js_name = sendObjectDatagramStatus)]
    pub async fn send_object_datagram_status(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_status: u8,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let object_status =
            ObjectStatus::try_from(object_status).map_err(|_| js_error("invalid object status"))?;
        let extension_headers = match crate::loc::parse_loc_header(loc_header) {
            Ok(Some(header)) => crate::loc::loc_header_to_extension_headers(&header),
            Ok(None) => Ok(empty_extension_headers()),
            Err(error) => Err(error),
        }
        .map_err(|error| js_error(error.to_string()))?;

        let field = if extension_headers == empty_extension_headers() {
            DatagramField::Status0x20 {
                object_id,
                publisher_priority,
                status: object_status,
            }
        } else {
            DatagramField::Status0x21 {
                object_id,
                publisher_priority,
                extension_headers,
                status: object_status,
            }
        };

        let payload = ObjectDatagram::new(track_alias, group_id, field).encode();
        self.send_datagram_bytes(&payload).await
    }

    #[wasm_bindgen(js_name = sendSubgroupHeader)]
    pub async fn send_subgroup_header(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<(), JsValue> {
        let writer = self
            .get_or_create_stream_writer(track_alias, group_id, subgroup_id)
            .await
            .map_err(|error| js_error(error.to_string()))?;
        let header = SubgroupHeader::new(
            track_alias,
            group_id,
            SubgroupId::Value(subgroup_id),
            publisher_priority,
            true,
            true,
        )
        .encode();
        write_to_writer(&writer, &header).await
    }

    #[wasm_bindgen(js_name = sendSubgroupObject)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_subgroup_object(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        object_number: u64,
        object_status: Option<u8>,
        object_payload: Vec<u8>,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let writer_key = (track_alias, group_id, subgroup_id);
        let writer = self
            .stream_writers
            .borrow()
            .get(&writer_key)
            .cloned()
            .ok_or_else(|| js_error("subgroup writer is None"))?;

        let extension_headers = match crate::loc::parse_loc_header(loc_header) {
            Ok(Some(header)) => crate::loc::loc_header_to_extension_headers(&header),
            Ok(None) => Ok(empty_extension_headers()),
            Err(error) => Err(error),
        }
        .map_err(|error| js_error(error.to_string()))?;
        let object_id_delta = {
            let stream_object_numbers = self.stream_object_numbers.borrow();
            match stream_object_numbers.get(&writer_key).copied() {
                Some(previous_object_number) => object_number.checked_sub(previous_object_number),
                None => object_number.checked_add(1),
            }
            .ok_or_else(|| {
                js_error("object number must be non-decreasing within a subgroup stream")
            })?
        };
        let header = SubgroupHeader::new(
            track_alias,
            group_id,
            SubgroupId::Value(subgroup_id),
            0,
            true,
            true,
        );

        let subgroup_object = match object_status {
            Some(status) => SubgroupObject::new_status(
                ObjectStatus::try_from(status).map_err(|_| js_error("invalid object status"))?
                    as u64,
            ),
            None => SubgroupObject::new_payload(Bytes::from(object_payload)),
        };

        let bytes = SubgroupObjectField {
            message_type: header.message_type,
            object_id_delta,
            extension_headers,
            subgroup_object,
        }
        .encode();

        write_to_writer(&writer, &bytes).await?;
        self.stream_object_numbers
            .borrow_mut()
            .insert(writer_key, object_number);

        if matches!(
            object_status.and_then(|status| ObjectStatus::try_from(status).ok()),
            Some(ObjectStatus::EndOfGroup | ObjectStatus::EndOfTrack)
        ) {
            let _ = JsFuture::from(writer.close()).await;
            self.stream_writers.borrow_mut().remove(&writer_key);
            self.stream_object_numbers.borrow_mut().remove(&writer_key);
        }

        Ok(())
    }

    pub async fn start(&self) -> Result<(), JsValue> {
        let transport = WebTransport::new(&self.url)?;
        *self.transport.borrow_mut() = Some(transport.clone());

        if let Err(error) = JsFuture::from(transport.ready()).await {
            self.transport.borrow_mut().take();
            return Err(error);
        }

        if let Err(error) = self.setup_transport(&transport).await {
            self.transport.borrow_mut().take();
            return Err(error);
        }

        Ok(())
    }

    #[wasm_bindgen(js_name = close)]
    pub async fn close(&self) -> Result<(), JsValue> {
        if let Some(transport) = self.transport.borrow().clone() {
            let closed = webtransport_closed_promise(&transport);
            transport.close();
            if let Some(closed) = closed {
                let _ = JsFuture::from(closed).await;
            }
        }
        self.transport.borrow_mut().take();
        self.control_stream_writer.borrow_mut().take();
        self.datagram_writer.borrow_mut().take();
        self.stream_writers.borrow_mut().clear();
        self.stream_object_numbers.borrow_mut().clear();
        Ok(())
    }

    async fn setup_transport(&self, transport: &WebTransport) -> Result<(), JsValue> {
        let callbacks = self.callbacks.clone();
        let transport_cell = self.transport.clone();
        if let Some(closed) = webtransport_closed_promise(transport) {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = JsFuture::from(closed).await;
                transport_cell.borrow_mut().take();
                if let Some(callback) = callbacks.borrow().connection_closed_callback.clone() {
                    let _ = callback.call0(&JsValue::NULL);
                }
            });
        }

        let control_stream = WebTransportBidirectionalStream::from(
            JsFuture::from(transport.create_bidirectional_stream()).await?,
        );
        let control_reader = ReadableStreamDefaultReader::new(&control_stream.readable().into())?;
        let control_writer = control_stream.writable().get_writer()?;
        *self.control_stream_writer.borrow_mut() = Some(control_writer);

        let datagram_writer = transport.datagrams().writable().get_writer()?;
        *self.datagram_writer.borrow_mut() = Some(datagram_writer);

        let callbacks = self.callbacks.clone();
        let state = self.state.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = control_stream_read_thread(callbacks, state, &control_reader).await;
        });

        let datagram_reader = ReadableStreamDefaultReader::new(&transport.datagrams().readable())?;
        let callbacks = self.callbacks.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = datagram_read_thread(callbacks, &datagram_reader).await;
        });

        let incoming_uni_streams = transport.incoming_unidirectional_streams();
        let incoming_uni_streams_reader = ReadableStreamDefaultReader::new(&incoming_uni_streams)?;
        let callbacks = self.callbacks.clone();
        *self.stream_writers.borrow_mut() = HashMap::new();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = receive_unidirectional_thread(callbacks, &incoming_uni_streams_reader).await;
        });

        Ok(())
    }

    async fn send_control_message(
        &self,
        message_type: ControlMessageType,
        payload: BytesMut,
    ) -> Result<(), JsValue> {
        let writer = self
            .control_stream_writer
            .borrow()
            .clone()
            .ok_or_else(|| js_error("control_stream_writer is None"))?;
        let bytes = encode_control_message(message_type, payload);
        write_to_writer(&writer, &bytes).await
    }

    async fn send_request_error(
        &self,
        message_type: ControlMessageType,
        request_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        self.send_control_message(
            message_type,
            RequestError {
                request_id,
                error_code,
                reason_phrase,
            }
            .encode(),
        )
        .await
    }

    async fn send_datagram_bytes(&self, payload: &[u8]) -> Result<(), JsValue> {
        let writer = self
            .datagram_writer
            .borrow()
            .clone()
            .ok_or_else(|| js_error("datagram_writer is None"))?;
        write_to_writer(&writer, payload).await
    }

    async fn get_or_create_stream_writer(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<WritableStreamDefaultWriter> {
        let writer_key = (track_alias, group_id, subgroup_id);
        if let Some(writer) = self.stream_writers.borrow().get(&writer_key).cloned() {
            return Ok(writer);
        }

        let transport = self
            .transport
            .borrow()
            .clone()
            .ok_or_else(|| anyhow!("transport is None"))?;
        let writable = web_sys::WritableStream::from(
            JsFuture::from(transport.create_unidirectional_stream())
                .await
                .map_err(|error| anyhow!("create_unidirectional_stream: {error:?}"))?,
        );
        let writer = writable
            .get_writer()
            .map_err(|error| anyhow!("get_writer: {error:?}"))?;
        self.stream_object_numbers.borrow_mut().remove(&writer_key);
        self.stream_writers
            .borrow_mut()
            .insert(writer_key, writer.clone());
        Ok(writer)
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn receive_unidirectional_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    while let Some(value) = read_reader_value(reader).await? {
        let stream = ReadableStream::from(value);
        let callbacks = callbacks.clone();
        let stream_reader = ReadableStreamDefaultReader::new(&stream)?;
        wasm_bindgen_futures::spawn_local(async move {
            let _ = uni_directional_stream_read_thread(callbacks, &stream_reader).await;
        });
    }
    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn control_stream_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    state: Rc<RefCell<ClientState>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    let mut buf = BytesMut::new();
    let mut stream_snapshot = Vec::new();

    while let Some(chunk) = read_byte_chunk(reader).await? {
        let new_bytes = normalize_stream_chunk(&mut stream_snapshot, chunk);
        if new_bytes.is_empty() {
            continue;
        }
        buf.extend_from_slice(&new_bytes);
        loop {
            match take_control_message(&mut buf).map_err(|error| js_error(error.to_string()))? {
                Some((message_type, payload)) => {
                    handle_control_message(callbacks.clone(), state.clone(), message_type, payload)
                        .await?;
                }
                None => break,
            }
        }
    }

    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn datagram_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    while let Some(chunk) = read_byte_chunk(reader).await? {
        let mut buf = BytesMut::from(chunk.as_slice());
        if let Some(datagram) = ObjectDatagram::decode(&mut buf) {
            emit_object_datagram(callbacks.clone(), datagram)?;
        }
    }
    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn uni_directional_stream_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    let mut header: Option<SubgroupHeader> = None;
    let mut buf = BytesMut::new();
    let mut stream_snapshot = Vec::new();

    while let Some(chunk) = read_byte_chunk(reader).await? {
        let new_bytes = normalize_stream_chunk(&mut stream_snapshot, chunk);
        if new_bytes.is_empty() {
            continue;
        }
        buf.extend_from_slice(&new_bytes);

        loop {
            if header.is_none() {
                match take_subgroup_header(&mut buf) {
                    Ok(Some(parsed_header)) => {
                        emit_subgroup_header(callbacks.clone(), &parsed_header)?;
                        header = Some(parsed_header);
                        continue;
                    }
                    Ok(None) => break,
                    Err(error) => return Err(js_error(error.to_string())),
                }
            }

            let parsed_header = header.clone().expect("subgroup header");
            match SubgroupObjectField::decode(parsed_header.message_type, &mut buf) {
                Ok(field) => {
                    let object_id_delta = field.object_id_delta;
                    emit_subgroup_object(
                        callbacks.clone(),
                        &parsed_header,
                        field,
                        object_id_delta,
                    )?;
                    continue;
                }
                Err(moqt::wire::DecodeError::NeedMoreData) => break,
                Err(moqt::wire::DecodeError::Fatal(error)) => return Err(js_error(error)),
            }
        }
    }

    let _ = JsFuture::from(reader.cancel()).await;
    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn handle_control_message(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    state: Rc<RefCell<ClientState>>,
    message_type: ControlMessageType,
    payload: BytesMut,
) -> Result<(), JsValue> {
    let mut cursor = Cursor::new(payload.as_ref());

    match message_type {
        ControlMessageType::ServerSetup => {
            let message = ServerSetup::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode SERVER_SETUP"))?;
            state
                .borrow_mut()
                .configure(message.setup_parameters.max_request_id);
            if let Some(callback) = callbacks.borrow().server_setup_callback.clone() {
                let wrapper = ServerSetupMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::PublishNamespace => {
            let message = PublishNamespace::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode PUBLISH_NAMESPACE"))?;
            if let Some(callback) = callbacks.borrow().publish_namespace_callback.clone() {
                let wrapper = PublishNamespaceMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::PublishNamespaceOk => {
            let message = NamespaceOk::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode PUBLISH_NAMESPACE_OK"))?;
            state
                .borrow_mut()
                .finish_publish_namespace_request(message.request_id, true);
            if let Some(callback) = callbacks
                .borrow()
                .publish_namespace_response_callback
                .clone()
            {
                let wrapper = NamespaceOkMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::PublishNamespaceError => {
            let message = RequestError::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode PUBLISH_NAMESPACE_ERROR"))?;
            state
                .borrow_mut()
                .finish_publish_namespace_request(message.request_id, false);
            if let Some(callback) = callbacks
                .borrow()
                .publish_namespace_response_callback
                .clone()
            {
                let wrapper = RequestErrorMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::SubscribeNamespaceOk => {
            let message = NamespaceOk::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode SUBSCRIBE_NAMESPACE_OK"))?;
            state
                .borrow_mut()
                .finish_subscribe_namespace_request(message.request_id, true);
            if let Some(callback) = callbacks
                .borrow()
                .subscribe_namespace_response_callback
                .clone()
            {
                let wrapper = NamespaceOkMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::SubscribeNamespaceError => {
            let message = RequestError::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode SUBSCRIBE_NAMESPACE_ERROR"))?;
            state
                .borrow_mut()
                .finish_subscribe_namespace_request(message.request_id, false);
            if let Some(callback) = callbacks
                .borrow()
                .subscribe_namespace_response_callback
                .clone()
            {
                let wrapper = RequestErrorMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::Publish => {
            let message =
                Publish::decode(&mut cursor).ok_or_else(|| js_error("failed to decode PUBLISH"))?;
            if let Some(callback) = callbacks.borrow().publish_callback.clone() {
                let wrapper = PublishMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::PublishOk => {
            let message = PublishOk::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode PUBLISH_OK"))?;
            state
                .borrow_mut()
                .finish_publish_request(message.request_id);
            if let Some(callback) = callbacks.borrow().publish_response_callback.clone() {
                let wrapper = PublishOkMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::PublishError => {
            let message = RequestError::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode PUBLISH_ERROR"))?;
            state
                .borrow_mut()
                .finish_publish_request(message.request_id);
            if let Some(callback) = callbacks.borrow().publish_response_callback.clone() {
                let wrapper = RequestErrorMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::Subscribe => {
            let message = Subscribe::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode SUBSCRIBE"))?;
            let validation_code = state.borrow().validate_incoming_subscribe(&message);
            if validation_code == 0 {
                state.borrow_mut().register_incoming_subscribe(&message);
            }
            if let Some(callback) = callbacks.borrow().subscribe_callback.clone() {
                let wrapper = SubscribeMessage::from(&message);
                let _ = callback.call3(
                    &JsValue::NULL,
                    &JsValue::from(wrapper),
                    &JsValue::from_bool(validation_code == 0),
                    &JsValue::from_f64(validation_code as f64),
                );
            }
        }
        ControlMessageType::SubscribeOk => {
            let message = SubscribeOk::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode SUBSCRIBE_OK"))?;
            state
                .borrow_mut()
                .activate_outgoing_subscription(message.request_id, message.track_alias);
            if let Some(callback) = callbacks.borrow().subscribe_response_callback.clone() {
                let wrapper = SubscribeOkMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::SubscribeError => {
            let message = RequestError::decode(&mut cursor)
                .ok_or_else(|| js_error("failed to decode SUBSCRIBE_ERROR"))?;
            state
                .borrow_mut()
                .remove_outgoing_subscription(message.request_id);
            if let Some(callback) = callbacks.borrow().subscribe_response_callback.clone() {
                let wrapper = RequestErrorMessage::from(&message);
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        ControlMessageType::UnSubscribe => {
            let request_id =
                decode_request_id(&mut cursor).map_err(|error| js_error(error.to_string()))?;
            state.borrow_mut().remove_incoming_subscribe(request_id);
            if let Some(callback) = callbacks.borrow().incoming_unsubscribe_callback.clone() {
                let _ = callback.call1(
                    &JsValue::NULL,
                    &JsValue::from(js_sys::BigInt::from(request_id)),
                );
            }
        }
        _ => {
            console_log!("Unhandled control message: {:?}", message_type);
        }
    }

    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
fn emit_object_datagram(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    datagram: ObjectDatagram,
) -> Result<(), JsValue> {
    match datagram.field {
        DatagramField::Payload0x00 {
            object_id,
            publisher_priority,
            payload,
        }
        | DatagramField::Payload0x02WithEndOfGroup {
            object_id,
            publisher_priority,
            payload,
        }
        | DatagramField::Payload0x04 {
            object_id,
            publisher_priority,
            payload,
        } => {
            if let Some(callback) = callbacks.borrow().object_datagram_callback.clone() {
                let wrapper = ObjectDatagramMessage::new(
                    datagram.track_alias,
                    datagram.group_id,
                    Some(object_id),
                    publisher_priority,
                    payload.to_vec(),
                    packages::loc::LocHeader::default(),
                );
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        DatagramField::Payload0x01 {
            object_id,
            publisher_priority,
            extension_headers,
            payload,
        }
        | DatagramField::Payload0x03WithEndOfGroup {
            object_id,
            publisher_priority,
            extension_headers,
            payload,
        } => {
            if let Some(callback) = callbacks.borrow().object_datagram_callback.clone() {
                let wrapper = ObjectDatagramMessage::new(
                    datagram.track_alias,
                    datagram.group_id,
                    Some(object_id),
                    publisher_priority,
                    payload.to_vec(),
                    crate::loc::extension_headers_to_loc_header(&extension_headers),
                );
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        DatagramField::Payload0x05 {
            publisher_priority,
            extension_headers,
            payload,
        }
        | DatagramField::Payload0x07WithEndOfGroup {
            publisher_priority,
            extension_headers,
            payload,
        } => {
            if let Some(callback) = callbacks.borrow().object_datagram_callback.clone() {
                let wrapper = ObjectDatagramMessage::new(
                    datagram.track_alias,
                    datagram.group_id,
                    None,
                    publisher_priority,
                    payload.to_vec(),
                    crate::loc::extension_headers_to_loc_header(&extension_headers),
                );
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        DatagramField::Payload0x06WithEndOfGroup {
            publisher_priority,
            payload,
        } => {
            if let Some(callback) = callbacks.borrow().object_datagram_callback.clone() {
                let wrapper = ObjectDatagramMessage::new(
                    datagram.track_alias,
                    datagram.group_id,
                    None,
                    publisher_priority,
                    payload.to_vec(),
                    packages::loc::LocHeader::default(),
                );
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        DatagramField::Status0x20 {
            object_id,
            publisher_priority,
            status,
        } => {
            if let Some(callback) = callbacks.borrow().object_datagram_status_callback.clone() {
                let wrapper = ObjectDatagramStatusMessage::new(
                    datagram.track_alias,
                    datagram.group_id,
                    Some(object_id),
                    publisher_priority,
                    status,
                    packages::loc::LocHeader::default(),
                );
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
        DatagramField::Status0x21 {
            object_id,
            publisher_priority,
            extension_headers,
            status,
        } => {
            if let Some(callback) = callbacks.borrow().object_datagram_status_callback.clone() {
                let wrapper = ObjectDatagramStatusMessage::new(
                    datagram.track_alias,
                    datagram.group_id,
                    Some(object_id),
                    publisher_priority,
                    status,
                    crate::loc::extension_headers_to_loc_header(&extension_headers),
                );
                let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
            }
        }
    }

    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
fn emit_subgroup_header(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    header: &SubgroupHeader,
) -> Result<(), JsValue> {
    if let Some(callback) = callbacks.borrow().subgroup_header_callback.clone() {
        let subgroup_id = match header.subgroup_id {
            SubgroupId::Value(value) => Some(value),
            _ => None,
        };
        let wrapper = SubgroupHeaderMessage::new(
            header.track_alias,
            header.group_id,
            subgroup_id,
            header.publisher_priority,
        );
        let _ = callback.call1(&JsValue::NULL, &JsValue::from(wrapper));
    }
    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
fn emit_subgroup_object(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    header: &SubgroupHeader,
    field: SubgroupObjectField,
    object_id_delta: u64,
) -> Result<(), JsValue> {
    if let Some(callback) = callbacks.borrow().subgroup_object_callback.clone() {
        let loc_header = crate::loc::extension_headers_to_loc_header(&field.extension_headers);
        let subgroup_id = match header.subgroup_id {
            SubgroupId::Value(value) => Some(value),
            _ => None,
        };
        let wrapper = match field.subgroup_object {
            SubgroupObject::Payload { data, .. } => SubgroupObjectMessage::new(
                subgroup_id,
                object_id_delta,
                None,
                data.to_vec(),
                loc_header,
            ),
            SubgroupObject::Status { code, .. } => {
                let status = ObjectStatus::try_from(code as u8).ok();
                SubgroupObjectMessage::new(
                    subgroup_id,
                    object_id_delta,
                    status,
                    Vec::new(),
                    loc_header,
                )
            }
        };
        let _ = callback.call3(
            &JsValue::NULL,
            &JsValue::from(js_sys::BigInt::from(header.track_alias)),
            &JsValue::from(js_sys::BigInt::from(header.group_id)),
            &JsValue::from(wrapper),
        );
    }
    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
fn default_setup_parameters(max_request_id: u64) -> SetupParameter {
    SetupParameter {
        path: None,
        max_request_id,
        authorization_token: vec![],
        max_auth_token_cache_size: None,
        authority: None,
        moq_implementation: Some("moqt-client-wasm".to_string()),
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
fn authorization_tokens(auth_info: &str) -> Vec<AuthorizationToken> {
    if auth_info.trim().is_empty() {
        return vec![];
    }

    vec![AuthorizationToken::UseValue {
        token_type: 0,
        token_value: Bytes::copy_from_slice(auth_info.as_bytes()),
    }]
}

#[cfg(feature = "web_sys_unstable_apis")]
fn empty_extension_headers() -> ExtensionHeaders {
    ExtensionHeaders {
        prior_group_id_gap: vec![],
        prior_object_id_gap: vec![],
        immutable_extensions: vec![],
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
fn content_exists_from_fields(
    content_exists: bool,
    largest_group_id: Option<u64>,
    largest_object_id: Option<u64>,
) -> ContentExists {
    if content_exists {
        ContentExists::True {
            location: Location {
                group_id: largest_group_id.unwrap_or(0),
                object_id: largest_object_id.unwrap_or(0),
            },
        }
    } else {
        ContentExists::False
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
fn filter_type_from_fields(
    filter_type: u8,
    start_group: Option<u64>,
    start_object: Option<u64>,
    end_group: Option<u64>,
) -> Result<FilterType, JsValue> {
    match filter_type {
        1 => Ok(FilterType::NextGroupStart),
        2 => Ok(FilterType::LargestObject),
        3 => Ok(FilterType::AbsoluteStart {
            location: Location {
                group_id: start_group.unwrap_or(0),
                object_id: start_object.unwrap_or(0),
            },
        }),
        4 => Ok(FilterType::AbsoluteRange {
            location: Location {
                group_id: start_group.unwrap_or(0),
                object_id: start_object.unwrap_or(0),
            },
            end_group: end_group.unwrap_or(0),
        }),
        _ => Err(js_error("invalid filter type")),
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
fn take_subgroup_header(buf: &mut BytesMut) -> Result<Option<SubgroupHeader>> {
    let mut cursor = Cursor::new(buf.as_ref());
    match SubgroupHeader::decode(&mut cursor) {
        Ok(header) => {
            buf.advance(cursor.position() as usize);
            Ok(Some(header))
        }
        Err(moqt::wire::DecodeError::NeedMoreData) => Ok(None),
        Err(moqt::wire::DecodeError::Fatal(error)) => Err(anyhow!(error)),
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
fn decode_request_id(cursor: &mut Cursor<&[u8]>) -> Result<u64> {
    cursor
        .try_get_varint()
        .map_err(|error| anyhow!("failed to decode request id: {error}"))
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn write_to_writer(
    writer: &WritableStreamDefaultWriter,
    bytes: &[u8],
) -> Result<(), JsValue> {
    let buffer = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
    buffer.copy_from(bytes);
    JsFuture::from(writer.write_with_chunk(&buffer)).await?;
    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn read_byte_chunk(reader: &ReadableStreamDefaultReader) -> Result<Option<Vec<u8>>, JsValue> {
    let result = JsFuture::from(reader.read()).await?;
    let done = js_sys::Boolean::from(js_sys::Reflect::get(&result, &JsValue::from_str("done"))?)
        .value_of();
    if done {
        return Ok(None);
    }
    let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))?;
    Ok(Some(js_sys::Uint8Array::from(value).to_vec()))
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn read_reader_value(
    reader: &ReadableStreamDefaultReader,
) -> Result<Option<JsValue>, JsValue> {
    let result = JsFuture::from(reader.read()).await?;
    let done = js_sys::Boolean::from(js_sys::Reflect::get(&result, &JsValue::from_str("done"))?)
        .value_of();
    if done {
        return Ok(None);
    }
    let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))?;
    Ok(Some(value))
}

#[cfg(feature = "web_sys_unstable_apis")]
fn normalize_stream_chunk(stream_snapshot: &mut Vec<u8>, chunk: Vec<u8>) -> Vec<u8> {
    if !stream_snapshot.is_empty()
        && chunk.len() >= stream_snapshot.len()
        && chunk.starts_with(stream_snapshot.as_slice())
    {
        let new_bytes = chunk[stream_snapshot.len()..].to_vec();
        *stream_snapshot = chunk;
        return new_bytes;
    }

    stream_snapshot.extend_from_slice(&chunk);
    chunk
}

#[cfg(feature = "web_sys_unstable_apis")]
fn webtransport_closed_promise(transport: &WebTransport) -> Option<js_sys::Promise> {
    js_sys::Reflect::get(transport.as_ref(), &JsValue::from_str("closed"))
        .ok()
        .and_then(|value| value.dyn_into::<js_sys::Promise>().ok())
}

#[cfg(feature = "web_sys_unstable_apis")]
fn js_error(message: impl Into<String>) -> JsValue {
    JsValue::from_str(&message.into())
}

#[cfg(feature = "web_sys_unstable_apis")]
#[derive(Default)]
struct MOQTCallbacks {
    server_setup_callback: Option<js_sys::Function>,
    publish_namespace_callback: Option<js_sys::Function>,
    publish_namespace_response_callback: Option<js_sys::Function>,
    subscribe_namespace_response_callback: Option<js_sys::Function>,
    publish_callback: Option<js_sys::Function>,
    publish_response_callback: Option<js_sys::Function>,
    subscribe_callback: Option<js_sys::Function>,
    subscribe_response_callback: Option<js_sys::Function>,
    incoming_unsubscribe_callback: Option<js_sys::Function>,
    object_datagram_callback: Option<js_sys::Function>,
    object_datagram_status_callback: Option<js_sys::Function>,
    subgroup_header_callback: Option<js_sys::Function>,
    subgroup_object_callback: Option<js_sys::Function>,
    connection_closed_callback: Option<js_sys::Function>,
}

#[cfg(not(feature = "web_sys_unstable_apis"))]
#[wasm_bindgen]
pub struct MOQTClient;

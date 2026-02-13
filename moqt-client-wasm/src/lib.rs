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
use anyhow::Result;
#[cfg(feature = "web_sys_unstable_apis")]
use bytes::{Buf, BufMut, BytesMut};
#[cfg(feature = "web_sys_unstable_apis")]
use js_sys::BigInt;
#[cfg(feature = "web_sys_unstable_apis")]
use moqt_core::{
    control_message_type::ControlMessageType,
    data_stream_type::DataStreamType,
    messages::control_messages::{
        announce::Announce,
        announce_error::AnnounceError,
        announce_ok::AnnounceOk,
        client_setup::ClientSetup,
        group_order::GroupOrder,
        server_setup::ServerSetup,
        setup_parameters::{MaxSubscribeID, SetupParameter},
        subscribe::{FilterType, Subscribe},
        subscribe_announces::SubscribeAnnounces,
        subscribe_announces_error::SubscribeAnnouncesError,
        subscribe_announces_ok::SubscribeAnnouncesOk,
        subscribe_done::SubscribeDone,
        subscribe_error::{SubscribeError, SubscribeErrorCode},
        subscribe_ok::SubscribeOk,
        unannounce::UnAnnounce,
        unsubscribe::Unsubscribe,
        version_specific_parameters::{
            AuthorizationInfo, DeliveryTimeout, MaxCacheDuration, VersionSpecificParameter,
        },
    },
    messages::{
        data_streams::{
            DataStreams, datagram, datagram_status, object_status::ObjectStatus, subgroup_stream,
        },
        moqt_payload::MOQTPayload,
    },
    models::subscriptions::{
        Subscription,
        nodes::{consumers::Consumer, producers::Producer, registry::SubscriptionNodeRegistry},
    },
    variable_integer::{
        read_variable_integer, read_variable_integer_from_buffer, write_variable_integer,
    },
};

#[cfg(feature = "web_sys_unstable_apis")]
use std::{cell::RefCell, collections::HashMap, io::Cursor, rc::Rc};
use wasm_bindgen::prelude::*;
#[cfg(feature = "web_sys_unstable_apis")]
use wasm_bindgen_futures::JsFuture;
#[cfg(feature = "web_sys_unstable_apis")]
use web_sys::{
    ReadableStream, ReadableStreamDefaultReader, WebTransport, WebTransportBidirectionalStream,
    WritableStream, WritableStreamDefaultWriter,
};

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);

    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[cfg(feature = "web_sys_unstable_apis")]
macro_rules! console_log {
    // Note that this is using the `log` function imported above during
    // `bare_bones`
    ($($t:tt)*) => (log(&format_args!($($t)*).to_string()))
}

// Call `utils::set_panic_hook` automatically
#[wasm_bindgen(start)]
fn main() {
    utils::set_panic_hook();
}

#[cfg(feature = "web_sys_unstable_apis")]
type SubscribeId = u64;
#[cfg(feature = "web_sys_unstable_apis")]
type GroupId = u64;
#[cfg(feature = "web_sys_unstable_apis")]
type SubgroupId = u64;
#[cfg(feature = "web_sys_unstable_apis")]
type WriterKey = (SubscribeId, Option<(GroupId, SubgroupId)>);

#[cfg(feature = "web_sys_unstable_apis")]
#[wasm_bindgen]
pub struct MOQTClient {
    pub id: u64,
    url: String,
    subscription_node: Rc<RefCell<SubscriptionNode>>,
    transport: Rc<RefCell<Option<WebTransport>>>,
    control_stream_writer: Rc<RefCell<Option<WritableStreamDefaultWriter>>>,
    datagram_writer: Rc<RefCell<Option<WritableStreamDefaultWriter>>>,
    stream_writers: Rc<RefCell<HashMap<WriterKey, WritableStreamDefaultWriter>>>,
    callbacks: Rc<RefCell<MOQTCallbacks>>,
}

#[cfg(feature = "web_sys_unstable_apis")]
#[wasm_bindgen]
impl MOQTClient {
    #[wasm_bindgen(constructor)]
    pub fn new(url: String) -> Self {
        MOQTClient {
            id: 42,
            url,
            subscription_node: Rc::new(RefCell::new(SubscriptionNode::new())),
            transport: Rc::new(RefCell::new(None)),
            control_stream_writer: Rc::new(RefCell::new(None)),
            datagram_writer: Rc::new(RefCell::new(None)),
            stream_writers: Rc::new(RefCell::new(HashMap::new())),
            callbacks: Rc::new(RefCell::new(MOQTCallbacks::new())),
        }
    }
    pub fn url(&self) -> JsValue {
        JsValue::from_str(self.url.as_str())
    }

    #[wasm_bindgen(js_name = onSetup)]
    pub fn set_setup_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_setup_callback(callback);
    }

    #[wasm_bindgen(js_name = onAnnounce)]
    pub fn set_announce_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_announce_callback(callback);
    }

    #[wasm_bindgen(js_name = onAnnounceResponse)]
    pub fn set_announce_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_announce_response_callback(callback);
    }

    #[wasm_bindgen(js_name = onSubscribe)]
    pub fn set_subscribe_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_subscribe_callback(callback);
    }

    #[wasm_bindgen(js_name = onUnsubscribe)]
    pub fn set_unsubscribe_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_unsubscribe_callback(callback)
    }

    #[wasm_bindgen(js_name = onSubscribeResponse)]
    pub fn set_subscribe_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_subscribe_response_callback(callback);
    }
    #[wasm_bindgen(js_name = onSubscribeAnnouncesResponse)]
    pub fn set_subscribe_announces_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_subscribe_announces_response_callback(callback);
    }

    #[wasm_bindgen(js_name = onDatagramObject)]
    pub fn set_datagram_object_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_datagram_object_callback(callback);
    }

    #[wasm_bindgen(js_name = onDatagramObjectStatus)]
    pub fn set_datagram_object_status_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_datagram_object_status_callback(callback);
    }

    #[wasm_bindgen(js_name = onSubgroupStreamHeader)]
    pub fn set_subgroup_stream_header_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_subgroup_stream_header_callback(callback);
    }

    #[wasm_bindgen(js_name = onSubgroupStreamObject)]
    pub fn set_subgroup_stream_object_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_subgroup_stream_object_callback(callback);
    }

    #[wasm_bindgen(js_name = onConnectionClosed)]
    pub fn set_connection_closed_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_connection_closed_callback(callback);
    }

    #[wasm_bindgen(js_name = isConnected)]
    pub fn is_connected(&self) -> bool {
        self.transport.borrow().is_some()
    }

    #[wasm_bindgen(js_name = sendSetupMessage)]
    pub async fn send_setup_message(
        &mut self,
        versions: Vec<u64>,
        max_subscribe_id: u64,
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let versions = versions.iter().map(|v| *v as u32).collect::<Vec<u32>>();
            let setup_parameters = vec![SetupParameter::MaxSubscribeID(MaxSubscribeID::new(
                max_subscribe_id,
            ))];

            let client_setup_message = ClientSetup::new(versions, setup_parameters);
            let mut client_setup_message_buf = BytesMut::new();
            client_setup_message.packetize(&mut client_setup_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::ClientSetup) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(client_setup_message_buf.len() as u64));
            buf.extend(client_setup_message_buf);

            // send
            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);
            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                // Setup nodes along with the role
                Ok(_) => {
                    log(std::format!("sent: client_setup: {:#x?}", client_setup_message).as_str());
                    self.subscription_node
                        .borrow_mut()
                        .setup_as_publisher(max_subscribe_id);
                    self.subscription_node
                        .borrow_mut()
                        .setup_as_subscriber(max_subscribe_id);

                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    // TODO: auth
    #[wasm_bindgen(js_name = sendAnnounceMessage)]
    pub async fn send_announce_message(
        &self,
        track_namespace: Vec<String>,
        auth_info: String, // param[0]
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let auth_info_parameter =
                VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(auth_info));

            let already_announced = !self
                .subscription_node
                .borrow_mut()
                .register_publisher_namespace(track_namespace.clone());

            if already_announced {
                return Ok(());
            }

            let announce_message =
                Announce::new(track_namespace.clone(), vec![auth_info_parameter]);
            let mut announce_message_buf = BytesMut::new();
            announce_message.packetize(&mut announce_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Announce) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(announce_message_buf.len() as u64));
            buf.extend(announce_message_buf);

            // send
            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);
            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!("sent: announce: {:#x?}", announce_message).as_str());
                    Ok(())
                }
                Err(e) => {
                    self.subscription_node
                        .borrow_mut()
                        .unregister_publisher_namespace(track_namespace);
                    Err(e)
                }
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendAnnounceOkMessage)]
    pub async fn send_announce_ok_message(
        &self,
        track_namespace: Vec<String>,
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let announce_ok_message = AnnounceOk::new(track_namespace.clone());
            let mut announce_ok_message_buf = BytesMut::new();
            announce_ok_message.packetize(&mut announce_ok_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::AnnounceOk) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(announce_ok_message_buf.len() as u64));
            buf.extend(announce_ok_message_buf);

            // send
            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);
            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!("sent: announce_ok: {:#x?}", announce_ok_message).as_str());
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendUnannounceMessage)]
    pub async fn send_unannounce_message(
        &self,
        track_namespace: Vec<String>,
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            {
                let mut node = self.subscription_node.borrow_mut();
                if !node.publisher_has_namespace(&track_namespace) {
                    return Ok(());
                }
                let _ = node.unregister_publisher_namespace(track_namespace.clone());
            }

            let unannounce_message = UnAnnounce::new(track_namespace.clone());
            let mut unannounce_message_buf = BytesMut::new();
            unannounce_message.packetize(&mut unannounce_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::UnAnnounce) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(unannounce_message_buf.len() as u64));
            buf.extend(unannounce_message_buf);

            // send
            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);
            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!("sent: unannounce: {:#x?}", unannounce_message).as_str());
                    Ok(())
                }
                Err(e) => {
                    self.subscription_node
                        .borrow_mut()
                        .register_publisher_namespace(track_namespace);
                    Err(e)
                }
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    // tmp impl
    #[wasm_bindgen(js_name = sendSubscribeMessage)]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_subscribe_message(
        &self,
        subscribe_id: u64,
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        priority: u8,
        group_order: u8,
        filter_type: u8,
        start_group: u64,
        start_object: u64,
        end_group: u64,
        auth_info: String,
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            // This is equal to `Now example`
            let auth_info =
                VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(auth_info));

            let group_order = GroupOrder::try_from(group_order).unwrap();
            let filter_type = FilterType::try_from(filter_type).unwrap();
            let (start_group, start_object) = match filter_type {
                FilterType::LatestObject | FilterType::LatestGroup => (None, None),
                FilterType::AbsoluteStart | FilterType::AbsoluteRange => {
                    (Some(start_group), Some(start_object))
                }
            };
            let end_group = match filter_type {
                FilterType::LatestObject | FilterType::LatestGroup | FilterType::AbsoluteStart => {
                    None
                }
                FilterType::AbsoluteRange => Some(end_group),
            };

            let max_cache_duration =
                VersionSpecificParameter::MaxCacheDuration(MaxCacheDuration::new(1000000));
            let delivery_timeout =
                VersionSpecificParameter::DeliveryTimeout(DeliveryTimeout::new(100000));
            let version_specific_parameters = vec![auth_info, max_cache_duration, delivery_timeout];
            let subscribe_message = Subscribe::new(
                subscribe_id,
                track_alias,
                track_namespace.clone(),
                track_name.clone(),
                priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
                version_specific_parameters,
            )
            .unwrap();
            let mut subscribe_message_buf = BytesMut::new();
            subscribe_message.packetize(&mut subscribe_message_buf);

            let is_already_subscribed = !self.subscription_node.borrow_mut().start_subscription(
                subscribe_id,
                track_alias,
                track_namespace.clone(),
                track_name.clone(),
                priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
            );

            if is_already_subscribed {
                return Ok(());
            }

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Subscribe) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(subscribe_message_buf.len() as u64));
            buf.extend(subscribe_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!("sent: subscribe: {:#x?}", subscribe_message).as_str());
                    Ok(())
                }
                Err(e) => {
                    self.subscription_node
                        .borrow_mut()
                        .cancel_subscription(subscribe_id);
                    Err(e)
                }
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = isSubscribed)]
    pub fn is_subscribed(&self, subscribe_id: u64) -> bool {
        self.subscription_node.borrow().is_subscribing(subscribe_id)
    }

    #[wasm_bindgen(js_name = getTrackSubscribers)]
    pub fn get_track_subscribers(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Vec<u64> {
        self.subscription_node
            .borrow()
            .get_publishing_track_aliases(track_namespace, track_name)
    }

    #[wasm_bindgen(js_name = getSubgroupState)]
    pub fn get_subgroup_state(&self, track_alias: u64) -> SubgroupState {
        let mut node = self.subscription_node.borrow_mut();
        node.current_subgroup_state(track_alias)
    }

    #[wasm_bindgen(js_name = markSubgroupHeaderSent)]
    pub fn mark_subgroup_header_sent(&self, track_alias: u64) {
        self.subscription_node
            .borrow_mut()
            .mark_subgroup_header_sent(track_alias);
    }

    #[wasm_bindgen(js_name = incrementSubgroupObject)]
    pub fn increment_subgroup_object(&self, track_alias: u64) {
        self.subscription_node
            .borrow_mut()
            .increment_subgroup_object(track_alias);
    }

    #[wasm_bindgen(js_name = resetSubgroupState)]
    pub fn reset_subgroup_state(&self, track_alias: u64) {
        self.subscription_node
            .borrow_mut()
            .reset_subgroup_state(track_alias);
    }

    #[wasm_bindgen(js_name = sendSubscribeOkMessage)]
    pub async fn send_subscribe_ok_message(
        &self,
        subscribe_id: u64,
        expires: u64,
        auth_info: String,
        fowarding_preference: String,
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let auth_info =
                VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(auth_info));
            let subscription = self
                .subscription_node
                .borrow()
                .get_publishing_subscription(subscribe_id)
                .unwrap();

            let requested_group_order = subscription.get_group_order();
            let group_order = if requested_group_order == GroupOrder::Original {
                // If requested_group_order is Original, use Ascending as its response
                GroupOrder::Ascending
            } else {
                // Otherwise, return the requested_group_order as is
                requested_group_order
            };

            let version_specific_parameters = vec![auth_info];
            let subscribe_ok_message = SubscribeOk::new(
                subscribe_id,
                expires,
                group_order,
                false,
                None,
                None,
                version_specific_parameters,
            );
            let mut subscribe_ok_message_buf = BytesMut::new();
            subscribe_ok_message.packetize(&mut subscribe_ok_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeOk) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(subscribe_ok_message_buf.len() as u64));
            buf.extend(subscribe_ok_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!("sent: subscribe_ok: {:#x?}", subscribe_ok_message).as_str());
                    self.subscription_node
                        .borrow_mut()
                        .activate_as_publisher(subscribe_id);

                    match &*fowarding_preference {
                        // stream
                        "datagram" => {
                            let datagram_writer = self
                                .transport
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .datagrams()
                                .writable()
                                .get_writer()?;
                            *self.datagram_writer.borrow_mut() = Some(datagram_writer);
                        }
                        "track" => {
                            // Writer will be generated when sending in a new Subgroup Stream
                        }
                        "subgroup" => {
                            // Writer will be generated when sending in a new Subgroup Stream
                        }
                        _ => {}
                    }

                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendSubscribeAnnouncesMessage)]
    pub async fn send_subscribe_announces_message(
        &self,
        track_namespace_prefix: Vec<String>,
        auth_info: String,
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let auth_info =
                VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(auth_info));

            let already_subscribed = !self
                .subscription_node
                .borrow_mut()
                .register_subscriber_namespace_prefix(track_namespace_prefix.clone());

            if already_subscribed {
                return Ok(());
            }

            let version_specific_parameters = vec![auth_info];
            let subscribe_announces_message = SubscribeAnnounces::new(
                track_namespace_prefix.clone(),
                version_specific_parameters,
            );
            let mut subscribe_announces_message_buf = BytesMut::new();
            subscribe_announces_message.packetize(&mut subscribe_announces_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeAnnounces) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(
                subscribe_announces_message_buf.len() as u64,
            ));
            buf.extend(subscribe_announces_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!(
                        "sent: subscribe_announces: {:#x?}",
                        subscribe_announces_message
                    )
                    .as_str());
                    Ok(())
                }
                Err(e) => {
                    self.subscription_node
                        .borrow_mut()
                        .unregister_subscriber_namespace_prefix(track_namespace_prefix);
                    Err(e)
                }
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendSubscribeErrorMessage)]
    pub async fn send_subscribe_error_message(
        &self,
        subscribe_id: u64,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            // Find unused subscribe_id and track_alias automatically
            let valid_track_alias = self
                .subscription_node
                .borrow_mut()
                .create_valid_track_alias()
                .unwrap();
            let subscribe_error_message = SubscribeError::new(
                subscribe_id,
                SubscribeErrorCode::try_from(error_code as u8).unwrap(),
                reason_phrase,
                valid_track_alias,
            );
            let mut subscribe_error_message_buf = BytesMut::new();
            subscribe_error_message.packetize(&mut subscribe_error_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeError) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(
                subscribe_error_message_buf.len() as u64
            ));
            buf.extend(subscribe_error_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(
                        std::format!("sent: subscribe_error: {:#x?}", subscribe_error_message)
                            .as_str(),
                    );
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendUnsubscribeMessage)]
    pub async fn send_unsubscribe_message(&self, subscribe_id: u64) -> Result<(), JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let unsubscribe_message = Unsubscribe::new(subscribe_id);
            let mut unsubscribe_message_buf = BytesMut::new();
            unsubscribe_message.packetize(&mut unsubscribe_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::UnSubscribe) as u64,
            ));
            // Message Payload and Payload Length
            buf.extend(write_variable_integer(unsubscribe_message_buf.len() as u64));
            buf.extend(unsubscribe_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    self.subscription_node
                        .borrow_mut()
                        .cancel_subscription(subscribe_id);
                    log(std::format!("sent: unsubscribe: {:#x?}", unsubscribe_message).as_str());
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendDatagramObject)]
    pub async fn send_datagram_object(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_payload: Vec<u8>,
    ) -> Result<(), JsValue> {
        let writer = self.datagram_writer.borrow().clone();
        if let Some(writer) = writer {
            let extension_headers = vec![];
            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                object_payload,
            )
            .unwrap();
            let mut datagram_object_buf = BytesMut::new();
            datagram_object.packetize(&mut datagram_object_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(DataStreamType::ObjectDatagram) as u64,
            ));
            buf.extend(datagram_object_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);
            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!("sent: object id: {:#?}", object_id).as_str());
                    Ok(())
                }
                Err(e) => {
                    log(std::format!("err: {:?}", e).as_str());
                    Err(e)
                }
            }
        } else {
            Err(JsValue::from_str("datagram_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendDatagramObjectStatus)]
    pub async fn send_datagram_object_status(
        &self,
        track_alias: u64,
        group_id: u64,
        object_id: u64,
        publisher_priority: u8,
        object_status: u8,
    ) -> Result<(), JsValue> {
        let writer = self.datagram_writer.borrow().clone();
        if let Some(writer) = writer {
            let extension_headers = vec![];
            let datagram_object = datagram_status::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                ObjectStatus::try_from(object_status).unwrap(),
            )
            .unwrap();
            let mut datagram_object_buf = BytesMut::new();
            datagram_object.packetize(&mut datagram_object_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(DataStreamType::ObjectDatagramStatus) as u64,
            ));
            buf.extend(datagram_object_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);
            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    log(std::format!("sent: object id: {:#?}", object_id).as_str());
                    Ok(())
                }
                Err(e) => {
                    log(std::format!("err: {:?}", e).as_str());
                    Err(e)
                }
            }
        } else {
            Err(JsValue::from_str("datagram_writer is None"))
        }
    }

    async fn get_or_create_stream_writer(
        &self,
        subscribe_id: u64,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<WritableStreamDefaultWriter> {
        let writer_key = (subscribe_id, Some((group_id, subgroup_id)));
        let mut need_create = false;

        // step 1: writer exists check
        {
            let stream_writers = self.stream_writers.borrow();
            if !stream_writers.contains_key(&writer_key) {
                need_create = true;
            }
        }
        // step 2: create if needed
        if need_create {
            let uni_stream_future = {
                let transport = self.transport.borrow();
                transport.as_ref().unwrap().create_unidirectional_stream()
            };
            let send_uni_stream = WritableStream::from(
                JsFuture::from(uni_stream_future)
                    .await
                    .map_err(|e| anyhow::Error::msg(format!("{:?}", e)))?,
            );
            let send_uni_stream_writer = match send_uni_stream.get_writer() {
                Ok(writer) => writer,
                Err(e) => return Err(anyhow::anyhow!("Failed to get writer: {:?}", e)),
            };

            self.stream_writers
                .borrow_mut()
                .insert(writer_key, send_uni_stream_writer);
        }
        // step 3: return writer
        let writer = {
            let stream_writers = self.stream_writers.borrow();
            stream_writers
                .get(&writer_key)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Writer not found for key: {:?}", writer_key))?
        };

        Ok(writer)
    }

    #[wasm_bindgen(js_name = sendSubgroupStreamHeaderMessage)]
    pub async fn send_subgroup_stream_header_message(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<(), JsValue> {
        let subscribe_id = match self
            .subscription_node
            .borrow()
            .get_publishing_subscribe_id_by_track_alias(track_alias)
        {
            Some(id) => id,
            None => return Err(JsValue::from_str("subscribe_id not found for track_alias")),
        };

        let writer = self
            .get_or_create_stream_writer(subscribe_id, group_id, subgroup_id)
            .await
            .map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;

        let subgroup_stream_header_message =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();
        let mut subgroup_stream_header_message_buf = BytesMut::new();
        subgroup_stream_header_message.packetize(&mut subgroup_stream_header_message_buf);

        let mut buf = Vec::new();
        // Message Type
        buf.extend(write_variable_integer(
            u8::from(DataStreamType::SubgroupHeader) as u64,
        ));
        buf.extend(subgroup_stream_header_message_buf);

        let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
        buffer.copy_from(&buf);
        JsFuture::from(writer.write_with_chunk(&buffer))
            .await
            .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = sendSubgroupStreamObject)]
    pub async fn send_subgroup_stream_object(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
        object_status: Option<u8>,
        object_payload: Vec<u8>,
        loc_header: JsValue,
    ) -> Result<(), JsValue> {
        let subscribe_id = match self
            .subscription_node
            .borrow()
            .get_publishing_subscribe_id_by_track_alias(track_alias)
        {
            Some(id) => id,
            None => return Err(JsValue::from_str("subscribe_id not found for track_alias")),
        };
        let writer_key = (subscribe_id, Some((group_id, subgroup_id)));
        let writer = {
            let stream_writers = self.stream_writers.borrow();
            stream_writers.get(&writer_key).cloned()
        };
        if let Some(writer) = writer {
            let extension_headers = match crate::loc::parse_loc_header(loc_header) {
                Ok(Some(loc_header)) => crate::loc::loc_header_to_extension_headers(&loc_header)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?,
                Ok(None) => vec![],
                Err(e) => return Err(JsValue::from_str(&e.to_string())),
            };
            let subgroup_stream_object = subgroup_stream::Object::new(
                object_id,
                extension_headers,
                object_status.map(|status| ObjectStatus::try_from(status).unwrap()),
                object_payload,
            )
            .unwrap();
            let mut subgroup_stream_object_buf = BytesMut::new();
            subgroup_stream_object.packetize(&mut subgroup_stream_object_buf);

            let mut buf = Vec::new();
            // Message Payload and Payload Length
            buf.extend(subgroup_stream_object_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);
            match JsFuture::from(writer.write_with_chunk(&buffer)).await {
                Ok(_) => {
                    // log(std::format!(
                    //     "sent: trackAlias: {:#?} object . group_id: {:#?} subgroup_id: {:#?} object_id: {:#?} object_status: {:#?}",
                    //     track_alias,
                    //     group_id,
                    //     subgroup_id,
                    //     object_id,
                    //     object_status,
                    // )
                    // .as_str());
                    Ok(())
                }
                Err(e) => {
                    log(std::format!("err: {:?}", e).as_str());
                    Err(e)
                }
            }
        } else {
            Err(JsValue::from_str("stream_writer is None"))
        }
    }

    pub async fn start(&self) -> Result<(), JsValue> {
        let transport = WebTransport::new(self.url.as_str());
        match &transport {
            Ok(v) => console_log!("{:#?}", v),
            Err(e) => {
                console_log!("{:#?}", e.as_string());
                return Err(e.clone());
            }
        }

        let transport = transport?;
        // Keep it for sending object messages
        *self.transport.borrow_mut() = Some(transport.clone());

        if let Err(err) = JsFuture::from(transport.ready()).await {
            self.transport.borrow_mut().take();
            return Err(err);
        }

        if let Err(err) = self.setup_transport(&transport).await {
            self.transport.borrow_mut().take();
            return Err(err);
        }

        Ok(())
    }

    async fn setup_transport(&self, transport: &WebTransport) -> Result<(), JsValue> {
        let callbacks = self.callbacks.clone();
        let transport_cell = self.transport.clone();
        let closed_promise = transport.closed();
        wasm_bindgen_futures::spawn_local(async move {
            let closed_result = JsFuture::from(closed_promise).await;
            transport_cell.borrow_mut().take();

            if let Some(callback) = callbacks.borrow().connection_closed_callback() {
                if let Err(error) = &closed_result {
                    console_log!("WebTransport closed with error: {:?}", error);
                }
                if let Err(err) = callback.call0(&JsValue::NULL) {
                    console_log!("connection_closed callback error: {:?}", err);
                }
            }
        });

        // All control messages are sent on same bidirectional stream which is called "control stream"
        let control_stream = WebTransportBidirectionalStream::from(
            JsFuture::from(transport.create_bidirectional_stream()).await?,
        );

        let control_stream_readable = control_stream.readable();
        let control_stream_reader =
            ReadableStreamDefaultReader::new(&control_stream_readable.into())?;

        let control_stream_writable = control_stream.writable();
        let control_stream_writer = control_stream_writable.get_writer()?;
        *self.control_stream_writer.borrow_mut() = Some(control_stream_writer);

        // For receiving control messages
        let callbacks = self.callbacks.clone();
        let subscription_node = self.subscription_node.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = bi_directional_stream_read_thread(
                callbacks,
                subscription_node,
                &control_stream_reader,
            )
            .await;
        });

        // For receiving object messages as datagrams
        let datagram_reader_readable = transport.datagrams().readable();
        let datagram_reader = ReadableStreamDefaultReader::new(&datagram_reader_readable)?;
        let callbacks = self.callbacks.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = datagram_read_thread(callbacks, &datagram_reader).await;
        });

        // For receiving object messages as streams
        let incoming_uni_stream = transport.incoming_unidirectional_streams();
        let incoming_uni_stream_reader = ReadableStreamDefaultReader::new(&incoming_uni_stream)?;
        let callbacks = self.callbacks.clone();
        *self.stream_writers.borrow_mut() = HashMap::new();

        wasm_bindgen_futures::spawn_local(async move {
            let _ = receive_unidirectional_thread(callbacks, &incoming_uni_stream_reader).await;
        });

        Ok(())
    }

    #[wasm_bindgen(js_name = close)]
    pub async fn close(&self) -> Result<(), JsValue> {
        let transport = self.transport.borrow().clone();
        if let Some(transport) = transport {
            transport.close();
            JsFuture::from(transport.closed()).await?;
        }
        Ok(())
    }

    pub fn array_buffer_sample_method(&self, buf: Vec<u8>) {
        log(std::format!("array_buffer_sample_method: {:#?}", buf).as_str());
    }
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn receive_unidirectional_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    log("receive_unidirectional_thread");

    loop {
        // Be careful about returned value of reader.read. It is a unidirectional stream of WebTransport.
        let ret = reader.read();
        let ret = JsFuture::from(ret).await?;

        let ret_value = js_sys::Reflect::get(&ret, &JsValue::from_str("value"))?;
        let ret_done = js_sys::Reflect::get(&ret, &JsValue::from_str("done"))?;
        let ret_done = js_sys::Boolean::from(ret_done).value_of();

        if ret_done {
            break;
        }

        let ret_value = ReadableStream::from(ret_value);

        let callbacks = callbacks.clone();

        let reader = ReadableStreamDefaultReader::new(&ret_value)?;
        wasm_bindgen_futures::spawn_local(async move {
            let _ = uni_directional_stream_read_thread(callbacks, &reader).await;
        });
    }

    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn bi_directional_stream_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    subscription_node: Rc<RefCell<SubscriptionNode>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    log("control_stream_read_thread");

    loop {
        let ret = reader.read();
        let ret = JsFuture::from(ret).await?;

        let ret_value = js_sys::Reflect::get(&ret, &JsValue::from_str("value"))?;
        let ret_done = js_sys::Reflect::get(&ret, &JsValue::from_str("done"))?;
        let ret_done = js_sys::Boolean::from(ret_done).value_of();

        if ret_done {
            break;
        }

        let ret_value = js_sys::Uint8Array::from(ret_value).to_vec();

        log(std::format!("bi: recv value: {} {:#?}", ret_value.len(), ret_value).as_str());

        let mut buf = BytesMut::with_capacity(ret_value.len());
        for i in ret_value {
            buf.put_u8(i);
        }

        while buf.has_remaining() {
            if let Err(e) =
                control_message_handler(callbacks.clone(), subscription_node.clone(), &mut buf)
                    .await
            {
                log(std::format!("error: {:#?}", e).as_str());
                break;
                // return Err(js_sys::Error::new(&e.to_string()).into());
            }
        }
    }

    Ok(())
}

// TODO: Separate handler to control message handler and object message handler
#[cfg(feature = "web_sys_unstable_apis")]
async fn control_message_handler(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    subscription_node: Rc<RefCell<SubscriptionNode>>,
    buf: &mut BytesMut,
) -> Result<()> {
    let message_type_value = read_variable_integer_from_buffer(buf);

    // TODO: Check stream type
    match message_type_value {
        Ok(v) => {
            let message_type = ControlMessageType::try_from(v as u8)?;
            let payload_length = read_variable_integer_from_buffer(buf)?;
            let mut payload_buf = buf.split_to(payload_length as usize);

            log(std::format!("message_type_value: {:#?}", message_type).as_str());

            match message_type {
                ControlMessageType::ServerSetup => {
                    let server_setup_message = ServerSetup::depacketize(&mut payload_buf)?;
                    log(
                        std::format!("recv: server_setup_message: {:#x?}", server_setup_message)
                            .as_str(),
                    );
                    if let Some(callback) = callbacks.borrow().setup_callback() {
                        let wrapper = ServerSetupMessage::from(&server_setup_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::Announce => {
                    let announce_message = Announce::depacketize(&mut payload_buf)?;
                    log(std::format!("recv: announce_message: {:#x?}", announce_message).as_str());

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        // Convert to JavaScript-friendly wrapper
                        let wrapper = AnnounceMessage::from(&announce_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::AnnounceOk => {
                    let announce_ok_message = AnnounceOk::depacketize(&mut payload_buf)?;
                    log(
                        std::format!("recv: announce_ok_message: {:#x?}", announce_ok_message)
                            .as_str(),
                    );

                    subscription_node
                        .borrow_mut()
                        .set_namespace(announce_ok_message.track_namespace().clone());

                    if let Some(callback) = callbacks.borrow().announce_response_callback() {
                        let wrapper = AnnounceOkMessage::from(&announce_ok_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::AnnounceError => {
                    let announce_error_message = AnnounceError::depacketize(&mut payload_buf)?;
                    log(std::format!(
                        "recv: announce_error_message: {:#x?}",
                        announce_error_message
                    )
                    .as_str());

                    if let Some(callback) = callbacks.borrow().announce_response_callback() {
                        let wrapper = AnnounceErrorMessage::from(&announce_error_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::Subscribe => {
                    let subscribe_message = Subscribe::depacketize(&mut payload_buf)?;
                    log(
                        std::format!("recv: subscribe_message: {:#x?}", subscribe_message).as_str(),
                    );

                    let result = subscription_node
                        .borrow_mut()
                        .validation_and_register_subscription(subscribe_message.clone());

                    if let Some(callback) = callbacks.borrow().subscribe_callback() {
                        let wrapper = SubscribeMessage::from(&subscribe_message);

                        match result {
                            Ok(_) => {
                                let is_success = JsValue::from_bool(true);
                                let empty = JsValue::null();
                                callback
                                    .call3(
                                        &JsValue::null(),
                                        &JsValue::from(wrapper),
                                        &(is_success),
                                        &(empty),
                                    )
                                    .unwrap();
                            }
                            Err(e) => {
                                let is_success = JsValue::from_bool(false);
                                if let Some(e_u8) = e.downcast_ref::<u8>() {
                                    let code = JsValue::from_f64(*e_u8 as f64);
                                    callback
                                        .call3(
                                            &JsValue::null(),
                                            &JsValue::from(wrapper),
                                            &(is_success),
                                            &(code),
                                        )
                                        .unwrap();
                                }
                            }
                        }
                    }
                }
                ControlMessageType::SubscribeOk => {
                    let subscribe_ok_message = SubscribeOk::depacketize(&mut payload_buf)?;
                    log(
                        std::format!("recv: subscribe_ok_message: {:#x?}", subscribe_ok_message)
                            .as_str(),
                    );

                    {
                        let mut node = subscription_node.borrow_mut();
                        node.mark_subscription_success(subscribe_ok_message.subscribe_id());
                        node.activate_as_subscriber(subscribe_ok_message.subscribe_id());
                    }

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let wrapper = SubscribeOkMessage::from(&subscribe_ok_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::SubscribeError => {
                    let subscribe_error_message = SubscribeError::depacketize(&mut payload_buf)?;
                    log(std::format!(
                        "recv: subscribe_error_message: {:#x?}",
                        subscribe_error_message
                    )
                    .as_str());

                    subscription_node
                        .borrow_mut()
                        .mark_subscription_failed(subscribe_error_message.subscribe_id());

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let wrapper = SubscribeErrorMessage::from(&subscribe_error_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::SubscribeDone => {
                    let subscribe_done_message = SubscribeDone::depacketize(&mut payload_buf)?;
                    log(std::format!(
                        "recv: subscribe_done_message: {:#x?}",
                        subscribe_done_message
                    )
                    .as_str());

                    subscription_node
                        .borrow_mut()
                        .cancel_subscription(subscribe_done_message.subscribe_id());

                    if let Some(callback) = callbacks.borrow().unsubscribe_callback() {
                        let wrapper = SubscribeDoneMessage::from(&subscribe_done_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::SubscribeAnnouncesOk => {
                    let subscribe_announces_ok_message =
                        SubscribeAnnouncesOk::depacketize(&mut payload_buf)?;
                    log(std::format!(
                        "recv: subscribe_announces_ok_message: {:#x?}",
                        subscribe_announces_ok_message
                    )
                    .as_str());

                    subscription_node.borrow_mut().set_namespace_prefix(
                        subscribe_announces_ok_message
                            .track_namespace_prefix()
                            .clone(),
                    );

                    if let Some(callback) =
                        callbacks.borrow().subscribe_announces_response_callback()
                    {
                        let wrapper =
                            SubscribeAnnouncesOkMessage::from(&subscribe_announces_ok_message);
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                ControlMessageType::SubscribeAnnouncesError => {
                    let subscribe_announces_error_message =
                        SubscribeAnnouncesError::depacketize(&mut payload_buf)?;
                    log(std::format!(
                        "recv: subscribe_announces_error_message: {:#x?}",
                        subscribe_announces_error_message
                    )
                    .as_str());

                    if let Some(callback) =
                        callbacks.borrow().subscribe_announces_response_callback()
                    {
                        let wrapper = SubscribeAnnouncesErrorMessage::from(
                            &subscribe_announces_error_message,
                        );
                        callback
                            .call1(&JsValue::null(), &JsValue::from(wrapper))
                            .unwrap();
                    }
                }
                _ => {
                    // TODO: impl rest of message type
                    log(std::format!("message_type: {:#?}", message_type).as_str());
                }
            };
        }
        Err(e) => {
            log("message_type_value is None");
            return Err(e);
        }
    }

    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn datagram_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    log("datagram_read_thread");

    let mut buf = BytesMut::new();

    loop {
        let ret = reader.read();
        let ret = JsFuture::from(ret).await?;

        let ret_value = js_sys::Reflect::get(&ret, &JsValue::from_str("value"))?;
        let ret_done = js_sys::Reflect::get(&ret, &JsValue::from_str("done"))?;
        let ret_done = js_sys::Boolean::from(ret_done).value_of();

        if ret_done {
            break;
        }

        let ret_value = js_sys::Uint8Array::from(ret_value).to_vec();

        for i in ret_value {
            buf.put_u8(i);
        }

        while !buf.is_empty() {
            if let Err(e) = datagram_handler(callbacks.clone(), &mut buf).await {
                log(std::format!("error: {:#?}", e).as_str());
                break;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn uni_directional_stream_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    log("uni_directional_stream_read_thread");

    let mut subgroup_stream_header: Option<subgroup_stream::Header> = None;
    let mut data_stream_type = DataStreamType::ObjectDatagram;
    let mut buf = BytesMut::new();
    let mut is_end_of_stream = false;

    while !is_end_of_stream {
        let ret = JsFuture::from(reader.read()).await?;
        let is_done =
            js_sys::Boolean::from(js_sys::Reflect::get(&ret, &JsValue::from_str("done"))?)
                .value_of();
        if is_done {
            break;
        }
        let value =
            js_sys::Uint8Array::from(js_sys::Reflect::get(&ret, &JsValue::from_str("value"))?)
                .to_vec();

        for i in value {
            buf.put_u8(i);
        }

        while !buf.is_empty() {
            if subgroup_stream_header.is_none() {
                let (_data_stream_type, _subgroup_stream_header) =
                    match object_header_handler(callbacks.clone(), &mut buf).await {
                        Ok(v) => v,
                        Err(_e) => {
                            break;
                        }
                    };
                data_stream_type = _data_stream_type;
                subgroup_stream_header = _subgroup_stream_header;
                continue;
            }

            match data_stream_type {
                DataStreamType::ObjectDatagram | DataStreamType::ObjectDatagramStatus => {
                    let msg = "format error".to_string();
                    log(std::format!("{:#?}", msg).as_str());
                    return Err(js_sys::Error::new(&msg).into());
                }
                DataStreamType::SubgroupHeader => {
                    match subgroup_stream_object_handler(
                        callbacks.clone(),
                        subgroup_stream_header.clone().unwrap(),
                        &mut buf,
                    )
                    .await
                    {
                        Ok(object) => {
                            if object.object_status() == Some(ObjectStatus::EndOfGroup) {
                                is_end_of_stream = true;
                                break;
                            }
                        }
                        Err(_e) => {
                            // log(std::format!("error: {:#?}", e).as_str());
                            break;
                        }
                    }
                }
                DataStreamType::FetchHeader => {
                    unimplemented!();
                }
            }
        }
    }
    JsFuture::from(reader.cancel()).await?;
    log("End of unidirectional stream");

    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn object_header_handler(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    buf: &mut BytesMut,
) -> Result<(DataStreamType, Option<subgroup_stream::Header>)> {
    let mut read_cur = Cursor::new(&buf[..]);
    let header_type_value = read_variable_integer(&mut read_cur);

    let (data_stream_type, subgroup_stream_header) = match header_type_value {
        Ok(v) => {
            let data_stream_type = DataStreamType::try_from(v as u8)?;

            log(std::format!("data_stream_type_value: {:#x?}", data_stream_type).as_str());

            let subgroup_stream_header = match data_stream_type {
                DataStreamType::SubgroupHeader => {
                    let subgroup_stream_header =
                        subgroup_stream::Header::depacketize(&mut read_cur)?;
                    buf.advance(read_cur.position() as usize);

                    log(
                        std::format!("subgroup_stream_header: {:#x?}", subgroup_stream_header)
                            .as_str(),
                    );

                    if let Some(callback) = callbacks.borrow().subgroup_stream_header_callback() {
                        let v = serde_wasm_bindgen::to_value(&subgroup_stream_header).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                    Some(subgroup_stream_header)
                }
                _ => {
                    // TODO: impl rest of message type
                    log(std::format!("data_stream_type: {:#?}", data_stream_type).as_str());
                    None
                }
            };

            (data_stream_type, subgroup_stream_header)
        }
        Err(e) => {
            log("data_stream_type_value is None");
            return Err(e);
        }
    };

    Ok((data_stream_type, subgroup_stream_header))
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn datagram_handler(callbacks: Rc<RefCell<MOQTCallbacks>>, buf: &mut BytesMut) -> Result<()> {
    use moqt_core::messages::data_streams::datagram_status;

    let mut read_cur = Cursor::new(&buf[..]);
    let header_type_value = read_variable_integer(&mut read_cur);

    match header_type_value {
        Ok(v) => {
            let data_stream_type = DataStreamType::try_from(v as u8)?;

            log(std::format!("data_stream_type_value: {:#x?}", data_stream_type).as_str());

            match data_stream_type {
                DataStreamType::ObjectDatagram => {
                    let datagram_object = match datagram::Object::depacketize(&mut read_cur) {
                        Ok(v) => {
                            buf.advance(read_cur.position() as usize);
                            v
                        }
                        Err(e) => {
                            read_cur.set_position(0);
                            log(std::format!("retry because: {:#?}", e).as_str());
                            return Err(e);
                        }
                    };

                    if let Some(callback) = callbacks.borrow().datagram_object_callback() {
                        let v = serde_wasm_bindgen::to_value(&datagram_object).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                DataStreamType::ObjectDatagramStatus => {
                    let datagram_object = match datagram_status::Object::depacketize(&mut read_cur)
                    {
                        Ok(v) => {
                            buf.advance(read_cur.position() as usize);
                            v
                        }
                        Err(e) => {
                            read_cur.set_position(0);
                            log(std::format!("retry because: {:#?}", e).as_str());
                            return Err(e);
                        }
                    };

                    if let Some(callback) = callbacks.borrow().datagram_object_status_callback() {
                        let v = serde_wasm_bindgen::to_value(&datagram_object).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                _ => {
                    let msg = "format error".to_string();
                    log(std::format!("msg: {}", msg).as_str());
                    return Err(anyhow::anyhow!(msg));
                }
            }
        }
        Err(e) => {
            log("data_stream_type_value is None");
            return Err(e);
        }
    }

    Ok(())
}

#[cfg(feature = "web_sys_unstable_apis")]
async fn subgroup_stream_object_handler(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    subgroup_stream_header: subgroup_stream::Header,
    buf: &mut BytesMut,
) -> Result<subgroup_stream::Object> {
    let mut read_cur = Cursor::new(&buf[..]);
    let subgroup_stream_object = match subgroup_stream::Object::depacketize(&mut read_cur) {
        Ok(v) => {
            buf.advance(read_cur.position() as usize);
            v
        }
        Err(e) => {
            read_cur.set_position(0);
            // log(std::format!("retry because: {:#?}", e).as_str());
            return Err(e);
        }
    };

    if let Some(callback) = callbacks.borrow().get_subgroup_stream_object_callback() {
        let wasm_object = SubgroupStreamObjectMessage::from(&subgroup_stream_object);
        let object_js = JsValue::from(wasm_object);
        let group_id = JsValue::from(BigInt::from(subgroup_stream_header.group_id()));
        let track_alias = JsValue::from(BigInt::from(subgroup_stream_header.track_alias()));
        callback
            .call3(&JsValue::NULL, &track_alias, &group_id, &object_js)
            .unwrap();
    }

    Ok(subgroup_stream_object)
}

#[cfg(feature = "web_sys_unstable_apis")]
struct SubscriptionNode {
    consumer: Option<Consumer>,
    producer: Option<Producer>,
    subgroup_states: HashMap<u64, SubgroupState>,
}

#[cfg(feature = "web_sys_unstable_apis")]
impl SubscriptionNode {
    fn new() -> Self {
        SubscriptionNode {
            consumer: None,
            producer: None,
            subgroup_states: HashMap::new(),
        }
    }

    fn setup_as_publisher(&mut self, max_subscribe_id: u64) {
        self.producer = Some(Producer::new(max_subscribe_id));
    }

    fn setup_as_subscriber(&mut self, max_subscribe_id: u64) {
        self.consumer = Some(Consumer::new(max_subscribe_id));
    }

    fn register_publisher_namespace(&mut self, namespace: Vec<String>) -> bool {
        if let Some(producer) = &mut self.producer {
            producer.set_namespace(namespace).is_ok()
        } else {
            false
        }
    }

    fn publisher_has_namespace(&self, namespace: &[String]) -> bool {
        if let Some(producer) = &self.producer {
            producer.has_namespace(namespace.to_vec())
        } else {
            false
        }
    }

    fn unregister_publisher_namespace(&mut self, namespace: Vec<String>) -> bool {
        if let Some(producer) = &mut self.producer
            && producer.has_namespace(namespace.clone())
        {
            let _ = producer.delete_namespace(namespace);
            return true;
        }
        false
    }

    fn register_subscriber_namespace_prefix(&mut self, namespace_prefix: Vec<String>) -> bool {
        if let Some(consumer) = &mut self.consumer {
            consumer.set_namespace_prefix(namespace_prefix).is_ok()
        } else {
            false
        }
    }

    fn unregister_subscriber_namespace_prefix(&mut self, namespace_prefix: Vec<String>) -> bool {
        if let Some(consumer) = &mut self.consumer
            && consumer
                .get_namespace_prefixes()
                .map(|prefixes| prefixes.contains(&namespace_prefix))
                .unwrap_or(false)
        {
            let _ = consumer.delete_namespace_prefix(namespace_prefix);
            return true;
        }
        false
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

    fn set_namespace(&mut self, track_namespace: Vec<String>) {
        if let Some(producer) = &mut self.producer {
            let _ = producer.set_namespace(track_namespace);
        }
    }

    fn set_namespace_prefix(&mut self, track_namespace_prefix: Vec<String>) {
        if let Some(consumer) = &mut self.consumer {
            let _ = consumer.set_namespace_prefix(track_namespace_prefix);
        }
    }

    fn create_valid_track_alias(&self) -> Result<u64> {
        if let Some(producer) = &self.producer {
            producer.create_valid_track_alias()
        } else {
            Err(anyhow::anyhow!("producer is None"))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_subscription(
        &mut self,
        subscribe_id: u64,
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
    ) -> bool {
        if let Some(consumer) = &mut self.consumer {
            if consumer
                .get_subscription(subscribe_id)
                .map(|option| option.is_some())
                .unwrap_or(false)
            {
                return false;
            }

            let _ = consumer.set_subscription(
                subscribe_id,
                track_alias,
                track_namespace,
                track_name,
                priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
            );
            true
        } else {
            false
        }
    }

    fn cancel_subscription(&mut self, subscribe_id: u64) {
        let track_alias = if let Some(consumer) = &mut self.consumer {
            let track_alias = consumer
                .get_subscription(subscribe_id)
                .ok()
                .and_then(|subscription| subscription.map(|s| s.get_track_alias()));
            let _ = consumer.delete_subscription(subscribe_id);
            track_alias
        } else {
            None
        };

        if let Some(alias) = track_alias {
            self.reset_subgroup_state(alias);
        }
    }

    fn mark_subscription_success(&mut self, subscribe_id: u64) {
        if let Some(consumer) = &mut self.consumer {
            let _ = consumer.set_subscription_success(subscribe_id);
        }
    }

    fn mark_subscription_failed(&mut self, subscribe_id: u64) {
        if let Some(consumer) = &mut self.consumer {
            let _ = consumer.set_subscription_failed(subscribe_id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn set_publishing_subscription(
        &mut self,
        subscribe_id: u64,
        track_alias: u64,
        track_namespace: Vec<String>,
        track_name: String,
        subscriber_priority: u8,
        group_order: GroupOrder,
        filter_type: FilterType,
        start_group: Option<u64>,
        start_object: Option<u64>,
        end_group: Option<u64>,
    ) {
        if let Some(producer) = &mut self.producer {
            let _ = producer.set_subscription(
                subscribe_id,
                track_alias,
                track_namespace,
                track_name,
                subscriber_priority,
                group_order,
                filter_type,
                start_group,
                start_object,
                end_group,
            );
        }
    }

    fn validate_subscribe(&self, subscribe_message: Subscribe) -> Result<()> {
        if let Some(producer) = &self.producer {
            match producer.has_namespace(subscribe_message.track_namespace().clone()) {
                true => {}
                false => {
                    let error_code = SubscribeErrorCode::TrackDoesNotExist;
                    return Err(anyhow::anyhow!(u8::from(error_code)));
                }
            }

            match producer.is_subscribe_id_unique(subscribe_message.subscribe_id()) {
                true => {}
                false => {
                    let error_code = SubscribeErrorCode::InvalidRange;
                    return Err(anyhow::anyhow!(u8::from(error_code)));
                }
            }

            match producer.is_subscribe_id_less_than_max(subscribe_message.subscribe_id()) {
                true => {}
                false => {
                    let error_code = SubscribeErrorCode::InvalidRange;
                    return Err(anyhow::anyhow!(u8::from(error_code)));
                }
            }

            match producer.is_track_alias_unique(subscribe_message.track_alias()) {
                true => {}
                false => {
                    let error_code = SubscribeErrorCode::RetryTrackAlias;
                    return Err(anyhow::anyhow!(u8::from(error_code)));
                }
            }
            Ok(())
        } else {
            let error_code = SubscribeErrorCode::InternalError;
            Err(anyhow::anyhow!(u8::from(error_code)))
        }
    }

    fn validation_and_register_subscription(&mut self, subscribe_message: Subscribe) -> Result<()> {
        self.validate_subscribe(subscribe_message.clone())?;

        self.set_publishing_subscription(
            subscribe_message.subscribe_id(),
            subscribe_message.track_alias(),
            subscribe_message.track_namespace().to_vec(),
            subscribe_message.track_name().to_string(),
            subscribe_message.subscriber_priority(),
            subscribe_message.group_order(),
            subscribe_message.filter_type(),
            subscribe_message.start_group(),
            subscribe_message.start_object(),
            subscribe_message.end_group(),
        );

        Ok(())
    }

    fn get_publishing_subscription(&self, subscribe_id: u64) -> Option<Subscription> {
        if let Some(producer) = &self.producer {
            producer.get_subscription(subscribe_id).unwrap()
        } else {
            None
        }
    }

    fn get_publishing_track_aliases(
        &self,
        track_namespace: Vec<String>,
        track_name: String,
    ) -> Vec<u64> {
        if let Some(producer) = &self.producer {
            producer
                .get_track_aliases_for_track(track_namespace, track_name)
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    fn activate_as_publisher(&mut self, subscribe_id: u64) {
        if let Some(producer) = &mut self.producer {
            let _ = producer.activate_subscription(subscribe_id);
        }
    }

    fn activate_as_subscriber(&mut self, subscribe_id: u64) {
        if let Some(consumer) = &mut self.consumer {
            let _ = consumer.activate_subscription(subscribe_id);
        }
    }

    fn get_publishing_subscribe_id_by_track_alias(&self, track_alias: u64) -> Option<u64> {
        if let Some(producer) = &self.producer {
            producer
                .get_subscribe_id_by_track_alias(track_alias)
                .unwrap()
        } else {
            None
        }
    }

    fn is_subscribing(&self, subscribe_id: u64) -> bool {
        if let Some(consumer) = &self.consumer {
            consumer.get_subscription(subscribe_id).unwrap().is_some()
        } else {
            false
        }
    }
}

// Due to the lifetime issue of `spawn_local`, it needs to be kept separate from MOQTClient.
// The callback is passed from JavaScript.
#[cfg(feature = "web_sys_unstable_apis")]
struct MOQTCallbacks {
    setup_callback: Option<js_sys::Function>,
    announce_callback: Option<js_sys::Function>,
    announce_response_callback: Option<js_sys::Function>,
    subscribe_callback: Option<js_sys::Function>,
    subscribe_response_callback: Option<js_sys::Function>,
    subscribe_announces_response_callback: Option<js_sys::Function>,
    unsubscribe_callback: Option<js_sys::Function>,
    datagram_object_callback: Option<js_sys::Function>,
    datagram_object_status_callback: Option<js_sys::Function>,
    subgroup_stream_header_callback: Option<js_sys::Function>,
    subgroup_stream_object_callback: Option<js_sys::Function>,
    connection_closed_callback: Option<js_sys::Function>,
}

#[cfg(feature = "web_sys_unstable_apis")]
impl MOQTCallbacks {
    fn new() -> Self {
        MOQTCallbacks {
            setup_callback: None,
            announce_callback: None,
            announce_response_callback: None,
            subscribe_callback: None,
            subscribe_response_callback: None,
            subscribe_announces_response_callback: None,
            unsubscribe_callback: None,
            datagram_object_callback: None,
            datagram_object_status_callback: None,
            subgroup_stream_header_callback: None,
            subgroup_stream_object_callback: None,
            connection_closed_callback: None,
        }
    }

    pub fn setup_callback(&self) -> Option<js_sys::Function> {
        self.setup_callback.clone()
    }

    pub fn set_setup_callback(&mut self, callback: js_sys::Function) {
        self.setup_callback = Some(callback);
    }

    pub fn announce_callback(&self) -> Option<js_sys::Function> {
        self.announce_callback.clone()
    }

    pub fn set_announce_callback(&mut self, callback: js_sys::Function) {
        self.announce_callback = Some(callback);
    }

    pub fn announce_response_callback(&self) -> Option<js_sys::Function> {
        self.announce_response_callback.clone()
    }

    pub fn set_announce_response_callback(&mut self, callback: js_sys::Function) {
        self.announce_response_callback = Some(callback);
    }

    pub fn subscribe_callback(&self) -> Option<js_sys::Function> {
        self.subscribe_callback.clone()
    }

    pub fn set_subscribe_callback(&mut self, callback: js_sys::Function) {
        self.subscribe_callback = Some(callback);
    }

    pub fn subscribe_response_callback(&self) -> Option<js_sys::Function> {
        self.subscribe_response_callback.clone()
    }

    pub fn set_subscribe_response_callback(&mut self, callback: js_sys::Function) {
        self.subscribe_response_callback = Some(callback);
    }

    pub fn subscribe_announces_response_callback(&self) -> Option<js_sys::Function> {
        self.subscribe_announces_response_callback.clone()
    }

    pub fn set_subscribe_announces_response_callback(&mut self, callback: js_sys::Function) {
        self.subscribe_announces_response_callback = Some(callback);
    }

    pub fn set_unsubscribe_callback(&mut self, callback: js_sys::Function) {
        self.unsubscribe_callback = Some(callback);
    }

    pub fn unsubscribe_callback(&self) -> Option<js_sys::Function> {
        self.unsubscribe_callback.clone()
    }

    pub fn datagram_object_callback(&self) -> Option<js_sys::Function> {
        self.datagram_object_callback.clone()
    }

    pub fn set_datagram_object_callback(&mut self, callback: js_sys::Function) {
        self.datagram_object_callback = Some(callback);
    }

    pub fn datagram_object_status_callback(&self) -> Option<js_sys::Function> {
        self.datagram_object_status_callback.clone()
    }

    pub fn set_datagram_object_status_callback(&mut self, callback: js_sys::Function) {
        self.datagram_object_status_callback = Some(callback);
    }

    pub fn subgroup_stream_header_callback(&self) -> Option<js_sys::Function> {
        self.subgroup_stream_header_callback.clone()
    }

    pub fn set_subgroup_stream_header_callback(&mut self, callback: js_sys::Function) {
        self.subgroup_stream_header_callback = Some(callback);
    }

    pub fn get_subgroup_stream_object_callback(&self) -> Option<js_sys::Function> {
        self.subgroup_stream_object_callback.clone()
    }

    pub fn set_subgroup_stream_object_callback(&mut self, callback: js_sys::Function) {
        self.subgroup_stream_object_callback = Some(callback);
    }

    pub fn connection_closed_callback(&self) -> Option<js_sys::Function> {
        self.connection_closed_callback.clone()
    }

    pub fn set_connection_closed_callback(&mut self, callback: js_sys::Function) {
        self.connection_closed_callback = Some(callback);
    }
}

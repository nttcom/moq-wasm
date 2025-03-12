mod utils;

#[cfg(feature = "web_sys_unstable_apis")]
use anyhow::Result;
#[cfg(feature = "web_sys_unstable_apis")]
use bytes::{Buf, BufMut, BytesMut};
#[cfg(feature = "web_sys_unstable_apis")]
use moqt_core::{
    control_message_type::ControlMessageType,
    data_stream_type::DataStreamType,
    messages::control_messages::{
        announce::Announce,
        announce_error::AnnounceError,
        announce_ok::AnnounceOk,
        client_setup::ClientSetup,
        server_setup::ServerSetup,
        setup_parameters::{MaxSubscribeID, SetupParameter},
        subscribe::{FilterType, GroupOrder, Subscribe},
        subscribe_announces::SubscribeAnnounces,
        subscribe_announces_error::SubscribeAnnouncesError,
        subscribe_announces_ok::SubscribeAnnouncesOk,
        subscribe_error::{SubscribeError, SubscribeErrorCode},
        subscribe_ok::SubscribeOk,
        unannounce::UnAnnounce,
        unsubscribe::Unsubscribe,
        version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
    },
    messages::{
        data_streams::{datagram, object_status::ObjectStatus, subgroup_stream, DataStreams},
        moqt_payload::MOQTPayload,
    },
    models::subscriptions::{
        nodes::{consumers::Consumer, producers::Producer, registry::SubscriptionNodeRegistry},
        Subscription,
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

    #[wasm_bindgen(js_name = onAnnounceResponce)]
    pub fn set_announce_responce_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_announce_responce_callback(callback);
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

    #[wasm_bindgen(js_name = onSubgroupStreamHeader)]
    pub fn set_subgroup_stream_header_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_subgroup_stream_header_callback(callback);
    }

    #[wasm_bindgen(js_name = onSubgroupStreamObject)]
    pub fn set_subgroup_stream_object_callback(
        &mut self,
        track_alias: u64,
        callback: js_sys::Function,
    ) {
        self.callbacks
            .borrow_mut()
            .set_subgroup_stream_object_callback(track_alias, callback);
    }

    #[wasm_bindgen(js_name = sendSetupMessage)]
    pub async fn send_setup_message(
        &mut self,
        versions: Vec<u64>,
        max_subscribe_id: u64,
    ) -> Result<JsValue, JsValue> {
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
                Ok(ok) => {
                    log(std::format!("sent: client_setup: {:#x?}", client_setup_message).as_str());
                    self.subscription_node
                        .borrow_mut()
                        .setup_as_publisher(max_subscribe_id);
                    self.subscription_node
                        .borrow_mut()
                        .setup_as_subscriber(max_subscribe_id);

                    Ok(ok)
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
        track_namespace: js_sys::Array,
        auth_info: String, // param[0]
    ) -> Result<JsValue, JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let auth_info_parameter =
                VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(auth_info));

            let length = track_namespace.length();
            let mut track_namespace_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_vec.push(string_element);
            }

            let announce_message =
                Announce::new(track_namespace_vec.clone(), vec![auth_info_parameter]);
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
                Ok(ok) => {
                    log(std::format!("sent: announce: {:#x?}", announce_message).as_str());
                    Ok(ok)
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendAnnounceOkMessage)]
    pub async fn send_announce_ok_message(
        &self,
        track_namespace: js_sys::Array,
    ) -> Result<JsValue, JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let length = track_namespace.length();
            let mut track_namespace_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_vec.push(string_element);
            }

            let announce_ok_message = AnnounceOk::new(track_namespace_vec.clone());
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
                Ok(ok) => {
                    log(std::format!("sent: announce_ok: {:#x?}", announce_ok_message).as_str());
                    Ok(ok)
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
        track_namespace: js_sys::Array,
    ) -> Result<JsValue, JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            // TODO: construct UnAnnounce
            let length = track_namespace.length();
            let mut track_namespace_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_vec.push(string_element);
            }

            let unannounce_message = UnAnnounce::new(track_namespace_vec);
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
                Ok(ok) => {
                    log(std::format!("sent: unannounce: {:#x?}", unannounce_message).as_str());
                    Ok(ok)
                }
                Err(e) => Err(e),
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
        track_namespace: js_sys::Array,
        track_name: String,
        priority: u8,
        group_order: u8,
        filter_type: u8,
        start_group: u64,
        start_object: u64,
        end_group: u64,
        auth_info: String,
    ) -> Result<JsValue, JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            // This is equal to `Now example`
            let auth_info =
                VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(auth_info));
            let length = track_namespace.length();
            let mut track_namespace_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_vec.push(string_element);
            }

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

            let version_specific_parameters = vec![auth_info];
            let subscribe_message = Subscribe::new(
                subscribe_id,
                track_alias,
                track_namespace_vec.clone(),
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
                Ok(ok) => {
                    log(std::format!("sent: subscribe: {:#x?}", subscribe_message).as_str());
                    // Register the subscribing track
                    self.subscription_node
                        .borrow_mut()
                        .set_subscribing_subscription(
                            subscribe_id,
                            track_alias,
                            track_namespace_vec,
                            track_name,
                            priority,
                            group_order,
                            filter_type,
                            start_group,
                            start_object,
                            end_group,
                        );

                    Ok(ok)
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendSubscribeOkMessage)]
    pub async fn send_subscribe_ok_message(
        &self,
        subscribe_id: u64,
        expires: u64,
        auth_info: String,
        fowarding_preference: String,
    ) -> Result<JsValue, JsValue> {
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
                Ok(ok) => {
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
                            let send_uni_stream = self
                                .transport
                                .borrow()
                                .as_ref()
                                .unwrap()
                                .create_unidirectional_stream();
                            let send_uni_stream =
                                WritableStream::from(JsFuture::from(send_uni_stream).await?);
                            let send_uni_stream_writer = send_uni_stream.get_writer()?;

                            let writer_key = (subscribe_id, None);
                            self.stream_writers
                                .borrow_mut()
                                .insert(writer_key, send_uni_stream_writer);
                        }
                        "subgroup" => {
                            // Writer will be generated when sending in a new Subgroup Stream
                        }
                        _ => {}
                    }

                    Ok(ok)
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
        track_namespace_prefix: js_sys::Array,
        auth_info: String,
    ) -> Result<JsValue, JsValue> {
        let writer = self.control_stream_writer.borrow().clone();
        if let Some(writer) = writer {
            let auth_info =
                VersionSpecificParameter::AuthorizationInfo(AuthorizationInfo::new(auth_info));
            let length = track_namespace_prefix.length();
            let mut track_namespace_prefix_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace_prefix.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_prefix_vec.push(string_element);
            }

            let version_specific_parameters = vec![auth_info];
            let subscribe_announces_message =
                SubscribeAnnounces::new(track_namespace_prefix_vec, version_specific_parameters);
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
                Ok(ok) => {
                    log(std::format!(
                        "sent: subscribe_announces: {:#x?}",
                        subscribe_announces_message
                    )
                    .as_str());
                    Ok(ok)
                }
                Err(e) => Err(e),
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
    ) -> Result<JsValue, JsValue> {
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
                Ok(ok) => {
                    log(
                        std::format!("sent: subscribe_error: {:#x?}", subscribe_error_message)
                            .as_str(),
                    );
                    Ok(ok)
                }
                Err(e) => Err(e),
            }
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendUnsubscribeMessage)]
    pub async fn send_unsubscribe_message(&self, subscribe_id: u64) -> Result<JsValue, JsValue> {
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
                Ok(ok) => {
                    log(std::format!("sent: unsubscribe: {:#x?}", unsubscribe_message).as_str());
                    Ok(ok)
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
    ) -> Result<JsValue, JsValue> {
        let writer = self.datagram_writer.borrow().clone();
        if let Some(writer) = writer {
            let extension_headers = vec![];
            let datagram_object = datagram::Object::new(
                track_alias,
                group_id,
                object_id,
                publisher_priority,
                extension_headers,
                None,
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
                Ok(ok) => {
                    log(std::format!("sent: object id: {:#?}", object_id).as_str());
                    Ok(ok)
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

    #[wasm_bindgen(js_name = sendSubgroupStreamHeaderMessage)]
    pub async fn send_subgroup_stream_header_message(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        publisher_priority: u8,
    ) -> Result<JsValue, JsValue> {
        let subscribe_id = self
            .subscription_node
            .borrow()
            .get_publishing_subscribe_id_by_track_alias(track_alias)
            .unwrap();
        let writer_key = (subscribe_id, Some((group_id, subgroup_id)));

        let writer = {
            let stream_writers = self.stream_writers.borrow_mut();

            if !stream_writers.contains_key(&writer_key) {
                // 新しいwriterを作成
                let _ = {
                    let transport = self.transport.borrow();
                    transport.as_ref().unwrap().create_unidirectional_stream()
                };
            };
            stream_writers.get(&writer_key).cloned()
        };
        let writer = if let Some(writer) = writer {
            writer
        } else {
            let uni_stream_future = {
                let transport = self.transport.borrow();
                transport.as_ref().unwrap().create_unidirectional_stream()
            };
            let send_uni_stream = WritableStream::from(JsFuture::from(uni_stream_future).await?);
            let send_uni_stream_writer = send_uni_stream.get_writer()?;

            // 作成したwriterを保存
            self.stream_writers
                .borrow_mut()
                .insert(writer_key, send_uni_stream_writer.clone());
            send_uni_stream_writer
        };

        let subgroup_stream_header_message =
            subgroup_stream::Header::new(track_alias, group_id, subgroup_id, publisher_priority)
                .unwrap();
        let mut subgroup_stream_header_message_buf = BytesMut::new();
        subgroup_stream_header_message.packetize(&mut subgroup_stream_header_message_buf);

        let mut buf = Vec::new();
        // Message Type
        buf.extend(write_variable_integer(
            u8::from(DataStreamType::StreamHeaderSubgroup) as u64,
        ));
        buf.extend(subgroup_stream_header_message_buf);

        let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
        buffer.copy_from(&buf);
        JsFuture::from(writer.write_with_chunk(&buffer)).await
    }

    #[wasm_bindgen(js_name = sendSubgroupStreamObject)]
    pub async fn send_subgroup_stream_object(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
        object_id: u64,
        object_status: Option<u8>,
        object_payload: Vec<u8>,
    ) -> Result<JsValue, JsValue> {
        let subscribe_id = self
            .subscription_node
            .borrow()
            .get_publishing_subscribe_id_by_track_alias(track_alias)
            .unwrap();
        let writer_key = (subscribe_id, Some((group_id, subgroup_id)));
        let writer = {
            let stream_writers = self.stream_writers.borrow();
            stream_writers.get(&writer_key).cloned()
        };
        if let Some(writer) = writer {
            let extension_headers = vec![];
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
                Ok(ok) => {
                    log(std::format!(
                        "sent: trackAlias: {:#?} object . group_id: {:#?} subgroup_id: {:#?} object_id: {:#?} object_status: {:#?}",
                        track_alias,
                        group_id,
                        subgroup_id,
                        object_id,
                        object_status,
                    )
                    .as_str());
                    Ok(ok)
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

    pub async fn start(&self) -> Result<JsValue, JsValue> {
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
        JsFuture::from(transport.ready()).await?;

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

        // // For receiving object messages as datagrams
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

        Ok(JsValue::null())
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
                        let v = serde_wasm_bindgen::to_value(&server_setup_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::Announce => {
                    let announce_message = Announce::depacketize(&mut payload_buf)?;
                    log(std::format!("recv: announce_message: {:#x?}", announce_message).as_str());

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
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

                    if let Some(callback) = callbacks.borrow().announce_responce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_ok_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::AnnounceError => {
                    let announce_error_message = AnnounceError::depacketize(&mut payload_buf)?;
                    log(std::format!(
                        "recv: announce_error_message: {:#x?}",
                        announce_error_message
                    )
                    .as_str());

                    if let Some(callback) = callbacks.borrow().announce_responce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_error_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
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
                        let v = serde_wasm_bindgen::to_value(&subscribe_message).unwrap();

                        match result {
                            Ok(_) => {
                                let is_success = JsValue::from_bool(true);
                                let empty = JsValue::null();
                                callback
                                    .call3(&JsValue::null(), &(v), &(is_success), &(empty))
                                    .unwrap();
                            }
                            Err(e) => {
                                let is_success = JsValue::from_bool(false);
                                if let Some(e_u8) = e.downcast_ref::<u8>() {
                                    let code = JsValue::from_f64(*e_u8 as f64);
                                    callback
                                        .call3(&JsValue::null(), &(v), &(is_success), &(code))
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

                    subscription_node
                        .borrow_mut()
                        .activate_as_subscriber(subscribe_ok_message.subscribe_id());

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_ok_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::SubscribeError => {
                    let subscribe_error_message = SubscribeError::depacketize(&mut payload_buf)?;
                    log(std::format!(
                        "recv: subscribe_error_message: {:#x?}",
                        subscribe_error_message
                    )
                    .as_str());

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_error_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
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
                        let v =
                            serde_wasm_bindgen::to_value(&subscribe_announces_ok_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
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
                        let v = serde_wasm_bindgen::to_value(&subscribe_announces_error_message)
                            .unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
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
                DataStreamType::ObjectDatagram => {
                    let msg = "format error".to_string();
                    log(std::format!("{:#?}", msg).as_str());
                    return Err(js_sys::Error::new(&msg).into());
                }
                DataStreamType::StreamHeaderSubgroup => {
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
                DataStreamType::StreamHeaderSubgroup => {
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
    let mut read_cur = Cursor::new(&buf[..]);
    let header_type_value = read_variable_integer(&mut read_cur);

    match header_type_value {
        Ok(v) => {
            let data_stream_type = DataStreamType::try_from(v as u8)?;

            log(std::format!("data_stream_type_value: {:#x?}", data_stream_type).as_str());

            if data_stream_type == DataStreamType::ObjectDatagram {
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
            } else {
                let msg = "format error".to_string();
                log(std::format!("msg: {}", msg).as_str());
                return Err(anyhow::anyhow!(msg));
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

    if let Some(callback) = callbacks
        .borrow()
        .get_subgroup_stream_object_callback(subgroup_stream_header.track_alias())
    {
        let v = serde_wasm_bindgen::to_value(&subgroup_stream_object).unwrap();
        callback.call1(&JsValue::null(), &(v)).unwrap();
    }

    Ok(subgroup_stream_object)
}

#[cfg(feature = "web_sys_unstable_apis")]
struct SubscriptionNode {
    consumer: Option<Consumer>,
    producer: Option<Producer>,
}

#[cfg(feature = "web_sys_unstable_apis")]
impl SubscriptionNode {
    fn new() -> Self {
        SubscriptionNode {
            consumer: None,
            producer: None,
        }
    }

    fn setup_as_publisher(&mut self, max_subscribe_id: u64) {
        self.producer = Some(Producer::new(max_subscribe_id));
    }

    fn setup_as_subscriber(&mut self, max_subscribe_id: u64) {
        self.consumer = Some(Consumer::new(max_subscribe_id));
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
    fn set_subscribing_subscription(
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
    ) {
        if let Some(consumer) = &mut self.consumer {
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
}

// Due to the lifetime issue of `spawn_local`, it needs to be kept separate from MOQTClient.
// The callback is passed from JavaScript.
#[cfg(feature = "web_sys_unstable_apis")]
struct MOQTCallbacks {
    setup_callback: Option<js_sys::Function>,
    announce_callback: Option<js_sys::Function>,
    announce_responce_callback: Option<js_sys::Function>,
    subscribe_callback: Option<js_sys::Function>,
    subscribe_response_callback: Option<js_sys::Function>,
    subscribe_announces_response_callback: Option<js_sys::Function>,
    unsubscribe_callback: Option<js_sys::Function>,
    datagram_object_callback: Option<js_sys::Function>,
    subgroup_stream_header_callback: Option<js_sys::Function>,
    subgroup_stream_object_callbacks: HashMap<u64, js_sys::Function>,
}

#[cfg(feature = "web_sys_unstable_apis")]
impl MOQTCallbacks {
    fn new() -> Self {
        MOQTCallbacks {
            setup_callback: None,
            announce_callback: None,
            announce_responce_callback: None,
            subscribe_callback: None,
            subscribe_response_callback: None,
            subscribe_announces_response_callback: None,
            unsubscribe_callback: None,
            datagram_object_callback: None,
            subgroup_stream_header_callback: None,
            subgroup_stream_object_callbacks: HashMap::new(),
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

    pub fn announce_responce_callback(&self) -> Option<js_sys::Function> {
        self.announce_responce_callback.clone()
    }

    pub fn set_announce_responce_callback(&mut self, callback: js_sys::Function) {
        self.announce_responce_callback = Some(callback);
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

    pub fn datagram_object_callback(&self) -> Option<js_sys::Function> {
        self.datagram_object_callback.clone()
    }

    pub fn set_datagram_object_callback(&mut self, callback: js_sys::Function) {
        self.datagram_object_callback = Some(callback);
    }

    pub fn subgroup_stream_header_callback(&self) -> Option<js_sys::Function> {
        self.subgroup_stream_header_callback.clone()
    }

    pub fn set_subgroup_stream_header_callback(&mut self, callback: js_sys::Function) {
        self.subgroup_stream_header_callback = Some(callback);
    }

    pub fn get_subgroup_stream_object_callback(
        &self,
        track_alias: u64,
    ) -> Option<&js_sys::Function> {
        let callback = self.subgroup_stream_object_callbacks.get(&track_alias);
        callback
    }

    pub fn set_subgroup_stream_object_callback(
        &mut self,
        track_alias: u64,
        callback: js_sys::Function,
    ) {
        self.subgroup_stream_object_callbacks
            .insert(track_alias, callback);
    }
}

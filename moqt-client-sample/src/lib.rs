mod utils;

use std::{cell::RefCell, rc::Rc};

use anyhow::{bail, Result};
use bytes::{Buf, BufMut, BytesMut};
use futures::join;
use moqt_core::{
    message_handler::StreamType,
    message_type::{self, MessageType},
    messages::{
        client_setup_message::{self, ClientSetupMessage},
        moqt_payload::MOQTPayload,
        setup_parameters::{RoleCase, RoleParameter, SetupParameter},
    },
    variable_bytes::write_variable_bytes,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::ReadableStreamDefaultReader;

// When the `wee_alloc` feature is enabled, use `wee_alloc` as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);

    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

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

#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
pub struct MOQTClient {
    pub id: u64,
    url: String,
    transport: Rc<RefCell<Option<web_sys::WebTransport>>>,
    control_stream_writer: Rc<RefCell<Option<web_sys::WritableStreamDefaultWriter>>>,
    callbacks: Rc<RefCell<MOQTCallbacks>>,
}

#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
impl MOQTClient {
    #[wasm_bindgen(constructor)]
    pub fn new(url: String) -> Self {
        MOQTClient {
            id: 42,
            url,
            transport: Rc::new(RefCell::new(None)),
            control_stream_writer: Rc::new(RefCell::new(None)),
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

    #[wasm_bindgen(js_name = onSubscribe)]
    pub fn set_subscribe_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_subscribe_callback(callback);
    }

    #[wasm_bindgen(js_name = onSubscribeResponse)]
    pub fn set_subscribe_response_callback(&mut self, callback: js_sys::Function) {
        self.callbacks
            .borrow_mut()
            .set_subscribe_response_callback(callback);
    }

    #[wasm_bindgen(js_name = onObject)]
    pub fn set_object_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_object_callback(callback);
    }

    #[wasm_bindgen(js_name = sendSetupMessage)]
    pub async fn send_setup_message(
        &self,
        role_value: u8,
        versions: Vec<u64>,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let role = RoleCase::try_from(role_value).unwrap();
            let versions = versions.iter().map(|v| *v as u32).collect::<Vec<u32>>();

            let client_setup_message = ClientSetupMessage::new(
                versions,
                vec![SetupParameter::RoleParameter(RoleParameter::new(role))],
            );
            let mut client_setup_message_buf = BytesMut::new();
            client_setup_message.packetize(&mut client_setup_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(MessageType::ClientSetup) as u64
            )); // client setup
            buf.extend(write_variable_integer(client_setup_message_buf.len() as u64)); // payload length
            buf.extend(client_setup_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    // TODO: auth
    #[wasm_bindgen(js_name = sendAnnounceMessage)]
    pub async fn send_announce_message(
        &self,
        track_name_space: String,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            // TODO: construct Announce Message
            let mut buf = Vec::new();
            buf.put_u8(0x06); // announce
            buf.extend(write_variable_integer(track_name_space.len() as u64 + 2)); // payload length
            buf.extend(write_variable_bytes(&track_name_space.as_bytes().to_vec()));
            buf.put_u8(0x00); // # of params

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendUnannounceMessage)]
    pub async fn send_unannounce_message(
        &self,
        track_name_space: String,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            // TODO: construct UnAnnounce Message
            let mut buf = Vec::new();
            buf.put_u8(0x09); // unannounce
            buf.extend(write_variable_integer(track_name_space.len() as u64 + 1)); // payload length
            buf.extend(write_variable_bytes(&track_name_space.as_bytes().to_vec()));

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    // tmp impl
    #[wasm_bindgen(js_name = sendSubscribeMessage)]
    pub async fn send_subscribe_message(
        &self,
        track_name_space: String,
        track_name: String,
        // start_group: Option<String>,
        // start_object: Option<String>,
        // end_group: Option<String>,
        // end_object: Option<String>,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            // This is equal to `Now example`
            let subscribe_message =
                moqt_core::messages::subscribe_request_message::SubscribeRequestMessage::new(
                    track_name_space,
                    track_name,
                    moqt_core::messages::subscribe_request_message::Location::RelativePrevious(0),
                    moqt_core::messages::subscribe_request_message::Location::Absolute(0),
                    moqt_core::messages::subscribe_request_message::Location::None,
                    moqt_core::messages::subscribe_request_message::Location::None,
                    Vec::new(),
                );
            let mut subscribe_message_buf = BytesMut::new();
            subscribe_message.packetize(&mut subscribe_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(MessageType::Subscribe) as u64
            )); // subscribe
            buf.extend(write_variable_integer(subscribe_message_buf.len() as u64)); // payload length
            buf.extend(subscribe_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendUnsubscribeMessage)]
    pub async fn send_unsubscribe_message(
        &self,
        track_name_space: String,
        track_name: String,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let unsubscribe_message =
                moqt_core::messages::unsubscribe_message::UnsubscribeMessage::new(
                    track_name_space,
                    track_name,
                );
            let mut unsubscribe_message_buf = BytesMut::new();
            unsubscribe_message.packetize(&mut unsubscribe_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(MessageType::UnSubscribe) as u64
            )); // unsubscribe
            buf.extend(write_variable_integer(unsubscribe_message_buf.len() as u64)); // payload length
            buf.extend(unsubscribe_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendObjectMessage)]
    pub async fn send_object_message(
        &self,
        track_id: u64,
        group_sequence: u64,
        object_sequence: u64,
        object_send_order: u64,
        object_payload: Vec<u8>,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            // Object message is sent on unidirectional stream
            let uni_stream = web_sys::WritableStream::from(
                JsFuture::from(
                    self.transport
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .create_unidirectional_stream(),
                )
                .await?,
            );
            let writer = uni_stream.get_writer()?;

            let object_message =
                moqt_core::messages::object_message::ObjectMessageWithPayloadLength::new(
                    track_id,
                    group_sequence,
                    object_sequence,
                    object_send_order,
                    object_payload,
                );
            let mut object_message_buf = BytesMut::new();
            object_message.packetize(&mut object_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(MessageType::ObjectWithLength) as u64,
            )); // object
            buf.extend(write_variable_integer(object_message_buf.len() as u64)); // payload length
            buf.extend(object_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            let result = JsFuture::from(writer.write_with_chunk(&buffer)).await;
            JsFuture::from(writer.close()).await;

            result
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendObjectMessageWithoutLength)]
    pub async fn send_object_message_without_length(
        &self,
        track_id: u64,
        group_sequence: u64,
        object_sequence: u64,
        object_send_order: u64,
        object_payload: Vec<u8>,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            // Object message is sent on unidirectional stream
            let uni_stream = web_sys::WritableStream::from(
                JsFuture::from(
                    self.transport
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .create_unidirectional_stream(),
                )
                .await?,
            );
            let writer = uni_stream.get_writer()?;

            let object_message =
                moqt_core::messages::object_message::ObjectMessageWithoutPayloadLength::new(
                    track_id,
                    group_sequence,
                    object_sequence,
                    object_send_order,
                    object_payload,
                );
            let mut object_message_buf = BytesMut::new();
            object_message.packetize(&mut object_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(MessageType::ObjectWithoutLength) as u64,
            )); // object
            buf.extend(write_variable_integer(object_message_buf.len() as u64)); // payload length
            buf.extend(object_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            let result = JsFuture::from(writer.write_with_chunk(&buffer)).await;
            JsFuture::from(writer.close()).await;

            result
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    pub async fn start(&self) -> Result<JsValue, JsValue> {
        let transport = web_sys::WebTransport::new(self.url.as_str());
        match &transport {
            Ok(v) => console_log!("{:#?}", v),
            Err(e) => {
                console_log!("{:#?}", e.as_string());
                return Err(e.clone());
            }
        }

        let transport = transport?;
        // Object送信時に使うので保持しておく
        *self.transport.borrow_mut() = Some(transport.clone());
        // await相当
        JsFuture::from(transport.ready()).await?;

        // All control messages are sent on same bidirectional stream which is called "control stream"
        let control_stream = web_sys::WebTransportBidirectionalStream::from(
            JsFuture::from(transport.create_bidirectional_stream()).await?,
        );

        let control_stream_readable = control_stream.readable();
        let control_stream_reader =
            web_sys::ReadableStreamDefaultReader::new(&control_stream_readable.into())?;

        let control_stream_writable = control_stream.writable();
        let control_stream_writer = control_stream_writable.get_writer()?;
        *self.control_stream_writer.borrow_mut() = Some(control_stream_writer);

        // For receiving control messages
        let callbacks = self.callbacks.clone();
        wasm_bindgen_futures::spawn_local(async move {
            stream_read_thread(callbacks, StreamType::Bi, &control_stream_reader).await;
        });

        // For receiving object messages
        let incoming_stream = transport.incoming_unidirectional_streams();
        let incoming_stream_reader =
            web_sys::ReadableStreamDefaultReader::new(&&incoming_stream.into())?;
        let callbacks = self.callbacks.clone();
        wasm_bindgen_futures::spawn_local(async move {
            receive_unidirectional_thread(callbacks, &incoming_stream_reader).await;
        });

        Ok(JsValue::null())
    }

    pub fn array_buffer_sample_method(&self, buf: Vec<u8>) {
        log(std::format!("array_buffer_sample_method: {:#?}", buf).as_str());
    }
}

#[cfg(web_sys_unstable_apis)]
async fn receive_unidirectional_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    log("receive_unidirectional_thread");

    loop {
        let ret = reader.read();
        let ret = JsFuture::from(ret).await?;

        let ret_value = js_sys::Reflect::get(&ret, &JsValue::from_str("value"))?;
        let ret_done = js_sys::Reflect::get(&ret, &JsValue::from_str("done"))?;
        let ret_done = js_sys::Boolean::from(ret_done).value_of();

        if ret_done {
            break;
        }

        let ret_value = web_sys::ReadableStream::from(ret_value);

        let callbacks = callbacks.clone();
        let reader = web_sys::ReadableStreamDefaultReader::new(&ret_value)?;
        wasm_bindgen_futures::spawn_local(async move {
            stream_read_thread(callbacks, StreamType::Uni, &reader).await;
        });
    }

    Ok(())
}

#[cfg(web_sys_unstable_apis)]
async fn stream_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    stream_type: StreamType,
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

        log(std::format!(
            "recv value: {:#?} {} {:#x?}",
            stream_type,
            ret_value.len(),
            ret_value
        )
        .as_str());

        let mut buf = BytesMut::with_capacity(ret_value.len());
        for i in ret_value {
            buf.put_u8(i);
        }

        if let Err(e) = message_handler(callbacks.clone(), stream_type.clone(), &mut buf).await {
            log(std::format!("error: {:#?}", e).as_str());
            return Err(js_sys::Error::new(&e.to_string()).into());
        }
    }

    Ok(())
}

#[cfg(web_sys_unstable_apis)]
async fn message_handler(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    stream_type: StreamType,
    mut buf: &mut BytesMut,
) -> Result<()> {
    // TODO: 読み戻しがあるかもしれないのでカーソルを使うようにする

    use moqt_core::messages::announce_ok_message;

    let message_type_value = read_variable_integer_from_buffer(&mut buf);

    let message_length = read_variable_integer_from_buffer(&mut buf)?;

    // TODO: Check stream type
    match message_type_value {
        Ok(v) => {
            let message_type = moqt_core::message_type::MessageType::try_from(v as u8)?;

            log(std::format!("message_type_value: {:#?}", message_type).as_str());

            match message_type {
                MessageType::ServerSetup => {
                    let server_setup_message =
                        moqt_core::messages::server_setup_message::ServerSetupMessage::depacketize(
                            &mut buf,
                        )?;

                    log(
                        std::format!("server_setup_message: {:#x?}", server_setup_message).as_str(),
                    );

                    if let Some(callback) = callbacks.borrow().setup_callback() {
                        callback
                            .call1(&JsValue::null(), &JsValue::from("called2"))
                            .unwrap();
                        // `.?`は型が違うのでコンパイルエラーになる
                        let v = serde_wasm_bindgen::to_value(&server_setup_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                MessageType::AnnounceOk => {
                    let announce_ok_message =
                        announce_ok_message::AnnounceOk::depacketize(&mut buf)?;

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_ok_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                MessageType::AnnounceError => {
                    let announce_error_message =
                        moqt_core::messages::announce_error_message::AnnounceError::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_error_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                MessageType::SubscribeOk => {
                    let subscribe_ok_message =
                        moqt_core::messages::subscribe_ok_message::SubscribeOk::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_ok_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                MessageType::SubscribeError => {
                    let subscribe_error_message =
                        moqt_core::messages::subscribe_error_message::SubscribeError::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_error_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                MessageType::ObjectWithLength => {
                    let object_with_length_message =
                        moqt_core::messages::object_message::ObjectMessageWithPayloadLength::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().object_callback() {
                        let v = serde_wasm_bindgen::to_value(&object_with_length_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                MessageType::ObjectWithoutLength => {
                    let object_without_length_message =
                        moqt_core::messages::object_message::ObjectMessageWithoutPayloadLength::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().object_callback() {
                        let v =
                            serde_wasm_bindgen::to_value(&object_without_length_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                _ => {
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

// lifetimeの問題のため、MOQTClientと別に持っておく必要がある
// callbackはJSから渡される
#[cfg(web_sys_unstable_apis)]
struct MOQTCallbacks {
    setup_callback: Option<js_sys::Function>,
    announce_callback: Option<js_sys::Function>,
    subscribe_callback: Option<js_sys::Function>,
    subscribe_response_callback: Option<js_sys::Function>,
    object_callback: Option<js_sys::Function>,
}

#[cfg(web_sys_unstable_apis)]
impl MOQTCallbacks {
    fn new() -> Self {
        MOQTCallbacks {
            setup_callback: None,
            announce_callback: None,
            subscribe_callback: None,
            subscribe_response_callback: None,
            object_callback: None,
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

    pub fn object_callback(&self) -> Option<js_sys::Function> {
        self.object_callback.clone()
    }

    pub fn set_object_callback(&mut self, callback: js_sys::Function) {
        self.object_callback = Some(callback);
    }
}

mod utils;

#[cfg(web_sys_unstable_apis)]
use anyhow::Result;
#[cfg(web_sys_unstable_apis)]
use bytes::{BufMut, BytesMut};
#[cfg(web_sys_unstable_apis)]
use moqt_core::{
    constants::StreamDirection,
    control_message_type::ControlMessageType,
    messages::control_messages::{
        announce::Announce,
        announce_error::AnnounceError,
        announce_ok::AnnounceOk,
        client_setup::ClientSetup,
        server_setup::ServerSetup,
        setup_parameters::{Role, RoleCase, SetupParameter},
        subscribe::{FilterType, GroupOrder, Subscribe},
        subscribe_error::{SubscribeError, SubscribeErrorCode},
        subscribe_ok::SubscribeOk,
        unannounce::UnAnnounce,
        unsubscribe::Unsubscribe,
        version_specific_parameters::{AuthorizationInfo, VersionSpecificParameter},
    },
    messages::moqt_payload::MOQTPayload,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer},
};
#[cfg(web_sys_unstable_apis)]
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::prelude::*;
#[cfg(web_sys_unstable_apis)]
use wasm_bindgen_futures::JsFuture;
#[cfg(web_sys_unstable_apis)]
use web_sys::ReadableStreamDefaultReader;

#[wasm_bindgen]
extern "C" {
    fn alert(s: &str);

    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);
}

#[cfg(web_sys_unstable_apis)]
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

    #[wasm_bindgen(js_name = sendSetupMessage)]
    pub async fn send_setup_message(
        &self,
        role_value: u8,
        versions: Vec<u64>,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let role = RoleCase::try_from(role_value).unwrap();
            let versions = versions.iter().map(|v| *v as u32).collect::<Vec<u32>>();

            let client_setup_message =
                ClientSetup::new(versions, vec![SetupParameter::Role(Role::new(role))]);
            let mut client_setup_message_buf = BytesMut::new();
            client_setup_message.packetize(&mut client_setup_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::ClientSetup) as u64,
            ));
            // Message Payload
            buf.extend(client_setup_message_buf);

            // send
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
        track_namespace: js_sys::Array,
        number_of_parameters: u8,
        auth_info: String, // param[0]
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
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

            let announce_message = Announce::new(
                track_namespace_vec,
                number_of_parameters,
                vec![auth_info_parameter],
            );
            let mut announce_message_buf = BytesMut::new();
            announce_message.packetize(&mut announce_message_buf);

            let mut buf = Vec::new();
            // Message Type
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Announce) as u64,
            ));
            // Message Payload
            buf.extend(announce_message_buf);

            // send
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
        track_namespace: js_sys::Array,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
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
            // Message Payload
            buf.extend(unannounce_message_buf);

            // send
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
        track_namespace: js_sys::Array,
        track_name: String,
        // start_group: Option<String>,
        // start_object: Option<String>,
        // end_group: Option<String>,
        // end_object: Option<String>,
        auth_info: String,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
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
            let version_specific_parameters = vec![auth_info];
            let subscribe_message = Subscribe::new(
                1,
                0,
                track_namespace_vec,
                track_name,
                1,
                GroupOrder::Ascending,
                FilterType::LatestGroup,
                None,
                None,
                None,
                None,
                version_specific_parameters,
            )
            .unwrap();
            let mut subscribe_message_buf = BytesMut::new();
            subscribe_message.packetize(&mut subscribe_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::Subscribe) as u64,
            )); // subscribe
            buf.extend(subscribe_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendSubscribeOkMessage)]
    pub async fn send_subscribe_ok_message(
        &self,
        track_namespace: js_sys::Array,
        track_name: String,
        track_id: u64,
        expires: u64,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let length = track_namespace.length();
            let mut track_namespace_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_vec.push(string_element);
            }
            let subscribe_ok_message =
                SubscribeOk::new(track_namespace_vec, track_name, track_id, expires);
            let mut subscribe_ok_message_buf = BytesMut::new();
            subscribe_ok_message.packetize(&mut subscribe_ok_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeOk) as u64,
            )); // subscribe ok
            buf.extend(subscribe_ok_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendSubscribeErrorMessage)]
    pub async fn send_subscribe_error_message(
        &self,
        track_namespace: js_sys::Array,
        track_name: String,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let length = track_namespace.length();
            let mut track_namespace_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_vec.push(string_element);
            }
            let subscribe_error_message = SubscribeError::new(
                track_namespace_vec,
                track_name,
                SubscribeErrorCode::try_from(error_code as u8).unwrap(),
                reason_phrase,
            );
            let mut subscribe_error_message_buf = BytesMut::new();
            subscribe_error_message.packetize(&mut subscribe_error_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::SubscribeError) as u64,
            )); // subscribe error
            buf.extend(subscribe_error_message_buf);

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
        track_namespace: js_sys::Array,
        track_name: String,
    ) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let length = track_namespace.length();
            let mut track_namespace_vec: Vec<String> = Vec::with_capacity(length as usize);
            for i in 0..length {
                let js_element = track_namespace.get(i);
                let string_element = js_element
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("Array contains a non-string element"))?;
                track_namespace_vec.push(string_element);
            }
            let unsubscribe_message = Unsubscribe::new(track_namespace_vec, track_name);
            let mut unsubscribe_message_buf = BytesMut::new();
            unsubscribe_message.packetize(&mut unsubscribe_message_buf);

            let mut buf = Vec::new();
            buf.extend(write_variable_integer(
                u8::from(ControlMessageType::UnSubscribe) as u64,
            )); // unsubscribe
            buf.extend(unsubscribe_message_buf);

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
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
        // Keep it for sending object messages
        *self.transport.borrow_mut() = Some(transport.clone());
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
            let _ =
                stream_read_thread(callbacks, StreamDirection::Bi, &control_stream_reader).await;
        });

        // For receiving object messages
        let incoming_stream = transport.incoming_unidirectional_streams();
        let incoming_stream_reader =
            web_sys::ReadableStreamDefaultReader::new(&&incoming_stream.into())?;
        let callbacks = self.callbacks.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = receive_unidirectional_thread(callbacks, &incoming_stream_reader).await;
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
        // Be careful about returned value of reader.read. It is a unidirectional stream of WebTransport.
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
            let _ = stream_read_thread(callbacks, StreamDirection::Uni, &reader).await;
        });
    }

    Ok(())
}

#[cfg(web_sys_unstable_apis)]
async fn stream_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    stream_direction: StreamDirection,
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
            stream_direction,
            ret_value.len(),
            ret_value
        )
        .as_str());

        let mut buf = BytesMut::with_capacity(ret_value.len());
        for i in ret_value {
            buf.put_u8(i);
        }

        if let Err(e) = message_handler(callbacks.clone(), stream_direction.clone(), &mut buf).await
        {
            log(std::format!("error: {:#?}", e).as_str());
            return Err(js_sys::Error::new(&e.to_string()).into());
        }
    }

    Ok(())
}

// TODO: Separate handler to control message handler and object message handler
#[cfg(web_sys_unstable_apis)]
async fn message_handler(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    _stream_direction: StreamDirection, // TODO: Not implemented yet
    mut buf: &mut BytesMut,
) -> Result<()> {
    let message_type_value = read_variable_integer_from_buffer(&mut buf);

    // TODO: Check stream type
    match message_type_value {
        Ok(v) => {
            let message_type = ControlMessageType::try_from(v as u8)?;

            log(std::format!("message_type_value: {:#?}", message_type).as_str());

            match message_type {
                ControlMessageType::ServerSetup => {
                    let server_setup_message = ServerSetup::depacketize(&mut buf)?;

                    log(
                        std::format!("server_setup_message: {:#x?}", server_setup_message).as_str(),
                    );

                    if let Some(callback) = callbacks.borrow().setup_callback() {
                        callback
                            .call1(&JsValue::null(), &JsValue::from("called2"))
                            .unwrap();
                        let v = serde_wasm_bindgen::to_value(&server_setup_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::AnnounceOk => {
                    let announce_ok_message = AnnounceOk::depacketize(&mut buf)?;
                    log(std::format!("announce_ok_message: {:#x?}", announce_ok_message).as_str());

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_ok_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::AnnounceError => {
                    let announce_error_message = AnnounceError::depacketize(&mut buf)?;
                    log(
                        std::format!("announce_error_message: {:#x?}", announce_error_message)
                            .as_str(),
                    );

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_error_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::Subscribe => {
                    let subscribe_message = Subscribe::depacketize(&mut buf)?;
                    log(std::format!("subscribe_message: {:#x?}", subscribe_message).as_str());

                    if let Some(callback) = callbacks.borrow().subscribe_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::SubscribeOk => {
                    let subscribe_ok_message = SubscribeOk::depacketize(&mut buf)?;
                    log(
                        std::format!("subscribe_ok_message: {:#x?}", subscribe_ok_message).as_str(),
                    );

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_ok_message).unwrap();
                        callback.call1(&JsValue::null(), &(v)).unwrap();
                    }
                }
                ControlMessageType::SubscribeError => {
                    let subscribe_error_message = SubscribeError::depacketize(&mut buf)?;
                    log(
                        std::format!("subscribe_error_message: {:#x?}", subscribe_error_message)
                            .as_str(),
                    );

                    if let Some(callback) = callbacks.borrow().subscribe_response_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_error_message).unwrap();
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

// Due to the lifetime issue of `spawn_local`, it needs to be kept separate from MOQTClient.
// The callback is passed from JavaScript.
#[cfg(web_sys_unstable_apis)]
struct MOQTCallbacks {
    setup_callback: Option<js_sys::Function>,
    announce_callback: Option<js_sys::Function>,
    subscribe_callback: Option<js_sys::Function>,
    subscribe_response_callback: Option<js_sys::Function>,
}

#[cfg(web_sys_unstable_apis)]
impl MOQTCallbacks {
    fn new() -> Self {
        MOQTCallbacks {
            setup_callback: None,
            announce_callback: None,
            subscribe_callback: None,
            subscribe_response_callback: None,
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
}

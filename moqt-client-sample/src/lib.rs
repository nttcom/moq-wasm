mod utils;

use std::{cell::RefCell, rc::Rc};

use anyhow::{bail, Result};
use bytes::{Buf, BufMut, BytesMut};
use futures::join;
use moqt_core::{
    message_type::{self, MessageType},
    messages::moqt_payload::MOQTPayload,
    variable_integer::{read_variable_integer_from_buffer, write_variable_integer}, variable_bytes::write_variable_bytes,
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

// #[wasm_bindgen]
// pub fn greet() {
//     alert("Hello, wasm-game-of-life!!");
// }

#[wasm_bindgen(start)]
fn main() {
    utils::set_panic_hook();
}

// #[derive(Deserialize)]
// struct ReadResult {
//     value: js_sys::Uint8Array,
//     done: bool,
// }


#[cfg(web_sys_unstable_apis)]
#[wasm_bindgen]
pub struct MOQTClient {
    pub id: u64,
    url: String,
    setup_callback: Option<js_sys::Function>,
    transport: Option<web_sys::WebTransport>,
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
            url: url.clone(),
            setup_callback: None,
            transport: None,
            control_stream_writer: Rc::new(RefCell::new(None)),
            callbacks: Rc::new(RefCell::new(MOQTCallbacks::new())),
        }
    }
    pub fn url(&self) -> JsValue {
        // log(std::format!("url: {}", self.url).as_str());
        JsValue::from_str(self.url.as_str())
        // JsValue::from(self.url)
        // JsValue::from("hello hello")
    }
    // 引数に、「u32を二つ引数に取りu32を返す関数」を受け取る
    pub fn set_and_exec_callback(&self, callback: js_sys::Function) {
        log(std::format!("callback: {:#?}", callback).as_str());
        // ここでcallbackを呼び出す
        callback
            .call2(&JsValue::null(), &JsValue::from(1), &JsValue::from(2))
            .unwrap();
    }

    #[wasm_bindgen(js_name = onSetupCallback)]
    pub fn set_setup_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_setup_callback(callback);
    }

    #[wasm_bindgen(js_name = onAnnounceCallback)]
    pub fn set_announce_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_announce_callback(callback);
    }

    #[wasm_bindgen(js_name = onSubscribeCallback)]
    pub fn set_subscribe_callback(&mut self, callback: js_sys::Function) {
        self.callbacks.borrow_mut().set_subscribe_callback(callback);
    }

    // TODO: Roleを設定できるようにする
    #[wasm_bindgen(js_name = sendSetupMessage)]
    pub async fn send_setup_message(&self) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let mut buf = Vec::new();
            buf.put_u16(0x4040); // client setup
            buf.put_u8(13); // length
            buf.put_u8(0x01); // # of versions
            buf.put_u64(0xc0000000ff000001); // version
            buf.put_u8(0x01); // # of params
            buf.put_u8(0x00); // role param
            buf.put_u8(0x01); // role value length
            buf.put_u8(0x01); // role value (injection)

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    // TODO: authをする
    #[wasm_bindgen(js_name = sendAnnounceMessage)]
    pub async fn send_announce_message(&self, track_name_space: String) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
            let mut buf = Vec::new();
            buf.put_u8(0x06); // announce
            buf.extend(write_variable_integer(track_name_space.len() as u64 + 2)); // payload length
            buf.extend(write_variable_bytes(&track_name_space.as_bytes().to_vec()));
            buf.put_u8(0x00); // # of params

            // log(std::format!("len: {:#?}", track_name_space.len()).as_str());
            // log(std::format!("track namespace: {:#?}", track_name_space).as_str());
            // log(std::format!("buf: {} {:#x?}", buf.len(), buf).as_str());

            let buffer = js_sys::Uint8Array::new_with_length(buf.len() as u32);
            buffer.copy_from(&buf);

            JsFuture::from(writer.write_with_chunk(&buffer)).await
        } else {
            Err(JsValue::from_str("control_stream_writer is None"))
        }
    }

    #[wasm_bindgen(js_name = sendUnannounceMessage)]
    pub async fn send_unannounce_message(&self, track_name_space: String) -> Result<JsValue, JsValue> {
        if let Some(writer) = &*self.control_stream_writer.borrow() {
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
            let subscribe_message = moqt_core::messages::subscribe_request_message::SubscribeRequestMessage::new(
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
            buf.put_u8(0x03); // subscribe
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
            let unsubscribe_message = moqt_core::messages::unsubscribe_message::UnsubscribeMessage::new(
                track_name_space,
                track_name,
            );
            let mut unsubscribe_message_buf = BytesMut::new();
            unsubscribe_message.packetize(&mut unsubscribe_message_buf);

            let mut buf = Vec::new();
            buf.put_u8(0x0a); // unsubscribe
            buf.extend(write_variable_integer(unsubscribe_message_buf.len() as u64)); // payload length
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
        // await相当
        JsFuture::from(transport.ready()).await?;

        let control_stream = web_sys::WebTransportBidirectionalStream::from(
            JsFuture::from(transport.create_bidirectional_stream()).await?,
        );

        let control_stream_readable = control_stream.readable();
        let control_stream_reader =
            web_sys::ReadableStreamDefaultReader::new(&control_stream_readable.into())?;

        let control_stream_writable = control_stream.writable();
        let control_stream_writer = control_stream_writable.get_writer()?;
        *self.control_stream_writer.borrow_mut() = Some(control_stream_writer);

        log("before join");

        let callbacks = self.callbacks.clone();
        wasm_bindgen_futures::spawn_local(
            async move  {
                control_stream_read_thread(callbacks, &control_stream_reader).await;
            }
        );

        log("after join");

        Ok(JsValue::null())
    }
}

// 関数を一つ引数に取るfという関数
// async fn f(g: fn() -> ()) {
async fn f(g: &dyn Fn()) {
    loop {
        g();
    }
}

#[cfg(web_sys_unstable_apis)]
async fn control_stream_read_thread(
    callbacks: Rc<RefCell<MOQTCallbacks>>,
    reader: &ReadableStreamDefaultReader,
) -> Result<(), JsValue> {
    log("control_stream_read_thread");

    loop {
        let ret = reader.read();
        let ret = JsFuture::from(ret).await?;

        // JsValueの中のobjectを取り出すコード
        let ret_value = js_sys::Reflect::get(&ret, &JsValue::from_str("value"))?;
        let ret_done = js_sys::Reflect::get(&ret, &JsValue::from_str("done"))?;
        // JsValueからUint8Arrayを取り出すコード
        // let ret_value = js_sys::Uint8Array::from(ret_value).value_of();
        let ret_value = js_sys::Uint8Array::from(ret_value).to_vec();
        // JsValueからboolを取り出すコード
        let ret_done = js_sys::Boolean::from(ret_done).value_of();

        if ret_done {
            break;
        }

        log(std::format!("recv value: {} {:#x?}", ret_value.len(), ret_value).as_str());

        let mut buf = BytesMut::with_capacity(ret_value.len());
        for i in ret_value {
            buf.put_u8(i);
        }

        if let Err(e) = control_message_handler(callbacks.clone(), &mut buf).await {
            log(std::format!("error: {:#?}", e).as_str());
            return Err(js_sys::Error::new(&e.to_string()).into());
        }
    }

    Ok(())
}

#[cfg(web_sys_unstable_apis)]
async fn control_message_handler(callbacks: Rc<RefCell<MOQTCallbacks>>, mut buf: &mut BytesMut) -> Result<()> {
    // TODO: 読み戻しがあるかもしれないのでカーソルを使うようにする

    use moqt_core::messages::announce_ok_message;

    let message_type_value = read_variable_integer_from_buffer(&mut buf);

    let message_length = read_variable_integer_from_buffer(&mut buf)?;

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

                    log(std::format!(
                        "server_setup_message: {:#x?}",
                        server_setup_message
                    ).as_str());

                    // if let Some(callback) = &self.setup_callback {
                    if let Some(callback) = callbacks.borrow().setup_callback() {
                        callback
                            .call1(&JsValue::null(), &JsValue::from("called2"))
                            .unwrap();
                        // `.?`は型が違うのでコンパイルエラーになる
                        let v = serde_wasm_bindgen::to_value(&server_setup_message).unwrap();
                        callback
                            .call1(&JsValue::null(), &(v))
                            .unwrap();
                    }
                }
                MessageType::AnnounceOk => {
                    let announce_ok_message =
                        announce_ok_message::AnnounceOk::depacketize(&mut buf)?;

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_ok_message).unwrap();
                        callback
                            .call1(&JsValue::null(), &(v))
                            .unwrap();
                    }
                }
                MessageType::AnnounceError => {
                    let announce_error_message =
                        moqt_core::messages::announce_error_message::AnnounceError::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().announce_callback() {
                        let v = serde_wasm_bindgen::to_value(&announce_error_message).unwrap();
                        callback
                            .call1(&JsValue::null(), &(v))
                            .unwrap();
                    }
                }
                MessageType::SubscribeOk => {
                    let subscribe_ok_message =
                        moqt_core::messages::subscribe_ok_message::SubscribeOk::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().subscribe_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_ok_message).unwrap();
                        callback
                            .call1(&JsValue::null(), &(v))
                            .unwrap();
                    }
                }
                MessageType::SubscribeError => {
                    let subscribe_error_message =
                        moqt_core::messages::subscribe_error_message::SubscribeError::depacketize(
                            &mut buf,
                        )?;

                    if let Some(callback) = callbacks.borrow().subscribe_callback() {
                        let v = serde_wasm_bindgen::to_value(&subscribe_error_message).unwrap();
                        callback
                            .call1(&JsValue::null(), &(v))
                            .unwrap();
                    }
                }
                _ => {
                    log(std::format!("message_type: {:#?}", message_type).as_str());
                }
            };
        }
        Err(e) => {
            log("message_type_value is None");
            // return Err(js_sys::Error::new(&e.to_string()));
            return Err(e);
        }
    }

    Ok(())
}

#[cfg(web_sys_unstable_apis)]
struct MOQTCallbacks {
    setup_callback: Option<js_sys::Function>,
    announce_callback: Option<js_sys::Function>,
    subscribe_callback: Option<js_sys::Function>,
}

#[cfg(web_sys_unstable_apis)]
impl MOQTCallbacks {
    fn new() -> Self {
        MOQTCallbacks {
            setup_callback: None,
            announce_callback: None,
            subscribe_callback: None,
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
}

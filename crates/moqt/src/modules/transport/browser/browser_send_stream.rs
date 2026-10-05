use async_trait::async_trait;
use bytes::BytesMut;
use js_sys::Uint8Array;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    WebTransportError, WebTransportErrorOptions, WebTransportSendStream,
    WritableStreamDefaultWriter,
};

use super::js_value::{is_session_error, js_error, stream_error_code};
use crate::modules::transport::transport_send_stream::{TransportSendError, TransportSendStream};

#[derive(Debug)]
pub struct BrowserSendStream {
    stream: WebTransportSendStream,
    writer: WritableStreamDefaultWriter,
}

impl BrowserSendStream {
    pub(crate) fn new(stream: WebTransportSendStream) -> anyhow::Result<Self> {
        let writer = stream.get_writer().map_err(js_error)?;
        Ok(Self { stream, writer })
    }
}

#[async_trait(?Send)]
impl TransportSendStream for BrowserSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError> {
        let chunk = Uint8Array::new_with_length(buffer.len() as u32);
        chunk.copy_from(buffer);
        JsFuture::from(self.writer.write_with_chunk(&chunk))
            .await
            .map(|_| ())
            .map_err(send_error)
    }

    async fn close(&mut self) -> Result<(), TransportSendError> {
        JsFuture::from(self.writer.close())
            .await
            .map(|_| ())
            .map_err(send_error)
    }

    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError> {
        let options = WebTransportErrorOptions::new();
        js_sys::Reflect::set(
            &options,
            &JsValue::from_str("streamErrorCode"),
            &JsValue::from_f64(error_code as f64),
        )
        .map_err(send_error)?;
        let reason = WebTransportError::new_with_message_and_options("reset", &options)
            .map_err(send_error)?;
        JsFuture::from(self.writer.abort_with_reason(&reason))
            .await
            .map(|_| ())
            .map_err(send_error)
    }

    /// WebTransport's `sendOrder` orders streams the same way as the QUIC
    /// stream priority: a higher value is sent first.
    fn set_priority(&mut self, priority: i32) -> Result<(), TransportSendError> {
        js_sys::Reflect::set(
            &self.stream,
            &JsValue::from_str("sendOrder"),
            &JsValue::from(priority),
        )
        .map(|_| ())
        .map_err(send_error)
    }
}

fn send_error(error: JsValue) -> TransportSendError {
    if let Some(code) = stream_error_code(&error) {
        TransportSendError::Stopped { code }
    } else if is_session_error(&error) {
        TransportSendError::SessionError {
            reason: format!("{error:?}"),
        }
    } else {
        TransportSendError::Transport {
            source: js_error(error),
        }
    }
}

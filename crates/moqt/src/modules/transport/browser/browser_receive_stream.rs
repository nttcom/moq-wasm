use std::{
    fmt::{self, Debug},
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::{Buf, BytesMut};
use js_sys::Uint8Array;
use tokio::io::ReadBuf;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStream, ReadableStreamDefaultReader, ReadableStreamReadResult};

use super::js_value::{is_session_error, js_error, stream_error_code};
use crate::modules::transport::{
    read_error::ReadError, transport_receive_stream::TransportReceiveStream,
};

pub struct BrowserReceiveStream {
    reader: ReadableStreamDefaultReader,
    pending_read: Option<JsFuture>,
    unread: BytesMut,
}

impl BrowserReceiveStream {
    pub(crate) fn new(stream: &ReadableStream) -> anyhow::Result<Self> {
        let reader = ReadableStreamDefaultReader::new(stream).map_err(js_error)?;
        Ok(Self {
            reader,
            pending_read: None,
            unread: BytesMut::new(),
        })
    }

    fn poll_next_chunk(&mut self, cx: &mut Context<'_>) -> Poll<Result<Option<Vec<u8>>, JsValue>> {
        let pending_read = self
            .pending_read
            .get_or_insert_with(|| JsFuture::from(self.reader.read()));
        let result = match Pin::new(pending_read).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        self.pending_read = None;
        let result: ReadableStreamReadResult = result?.unchecked_into();
        if result.get_done() == Some(true) {
            return Poll::Ready(Ok(None));
        }
        Poll::Ready(Ok(Some(Uint8Array::from(result.get_value()).to_vec())))
    }
}

impl Debug for BrowserReceiveStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserReceiveStream")
            .field("unread", &self.unread.len())
            .finish()
    }
}

impl TransportReceiveStream for BrowserReceiveStream {
    fn poll_read(
        &mut self,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), ReadError>> {
        loop {
            if !self.unread.is_empty() {
                let length = self.unread.len().min(buf.remaining());
                buf.put_slice(&self.unread[..length]);
                self.unread.advance(length);
                return Poll::Ready(Ok(()));
            }
            match self.poll_next_chunk(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(None)) => return Poll::Ready(Ok(())),
                Poll::Ready(Ok(Some(chunk))) => self.unread.extend_from_slice(&chunk),
                Poll::Ready(Err(error)) => return Poll::Ready(Err(read_error(&error))),
            }
        }
    }
}

fn read_error(error: &JsValue) -> ReadError {
    if stream_error_code(error).is_some() {
        ReadError::Reset
    } else if is_session_error(error) {
        ReadError::ConnectionLost
    } else {
        ReadError::Closed
    }
}

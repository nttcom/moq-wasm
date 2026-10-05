use anyhow::Context;
use async_trait::async_trait;
use bytes::BytesMut;
use js_sys::Uint8Array;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    ReadableStreamDefaultReader, WebTransport, WebTransportBidirectionalStream,
    WebTransportCloseInfo, WebTransportDatagramDuplexStream, WritableStream,
    WritableStreamDefaultWriter,
};

use super::{
    browser_receive_stream::BrowserReceiveStream,
    browser_send_stream::BrowserSendStream,
    js_value::{js_error, read_next},
};
use crate::modules::transport::{
    transport_connection::{TransportClose, TransportConnection},
    transport_stats::TransportStats,
};

#[derive(Debug)]
pub struct BrowserConnection {
    transport: WebTransport,
    incoming_bi_streams: ReadableStreamDefaultReader,
    incoming_uni_streams: ReadableStreamDefaultReader,
    datagram_reader: ReadableStreamDefaultReader,
    datagram_writer: Option<WritableStreamDefaultWriter>,
}

impl BrowserConnection {
    pub(crate) fn new(transport: WebTransport) -> anyhow::Result<Self> {
        let incoming_bi_streams =
            ReadableStreamDefaultReader::new(&transport.incoming_bidirectional_streams())
                .map_err(js_error)?;
        let incoming_uni_streams =
            ReadableStreamDefaultReader::new(&transport.incoming_unidirectional_streams())
                .map_err(js_error)?;
        let datagrams = transport.datagrams();
        let datagram_reader =
            ReadableStreamDefaultReader::new(&datagrams.readable()).map_err(js_error)?;
        let datagram_writer = datagram_writable(&datagrams)
            .map(|writable| writable.get_writer())
            .transpose()
            .map_err(js_error)?;
        Ok(Self {
            transport,
            incoming_bi_streams,
            incoming_uni_streams,
            datagram_reader,
            datagram_writer,
        })
    }

    fn split_bi(
        stream: WebTransportBidirectionalStream,
    ) -> anyhow::Result<(BrowserSendStream, BrowserReceiveStream)> {
        let send_stream = BrowserSendStream::new(stream.writable())?;
        let receive_stream = BrowserReceiveStream::new(&stream.readable())?;
        Ok((send_stream, receive_stream))
    }
}

#[async_trait(?Send)]
impl TransportConnection for BrowserConnection {
    type SendStream = BrowserSendStream;
    type ReceiveStream = BrowserReceiveStream;

    async fn closed(&self) -> TransportClose {
        let close = match JsFuture::from(self.transport.closed()).await {
            Ok(close_info) => TransportClose {
                code: js_sys::Reflect::get(&close_info, &JsValue::from_str("closeCode"))
                    .ok()
                    .and_then(|code| code.as_f64())
                    .map(|code| code as u32),
                reason: js_sys::Reflect::get(&close_info, &JsValue::from_str("reason"))
                    .ok()
                    .and_then(|reason| reason.as_string())
                    .unwrap_or_default(),
            },
            Err(error) => TransportClose {
                code: None,
                reason: format!("{error:?}"),
            },
        };
        tracing::info!(?close, "WebTransport connection closed");
        close
    }

    fn close(&self, code: u32, reason: &str) {
        let close_info = WebTransportCloseInfo::new();
        close_info.set_close_code(code);
        close_info.set_reason(reason);
        self.transport.close_with_close_info(&close_info);
        tracing::info!(code, reason, "WebTransport connection close requested");
    }

    async fn open_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let stream: WebTransportBidirectionalStream =
            JsFuture::from(self.transport.create_bidirectional_stream())
                .await
                .map_err(js_error)?
                .unchecked_into();
        Self::split_bi(stream)
    }

    async fn accept_bi(&self) -> anyhow::Result<(Self::SendStream, Self::ReceiveStream)> {
        let stream = read_next(&self.incoming_bi_streams)
            .await
            .map_err(js_error)?
            .context("incoming bidirectional streams ended")?;
        Self::split_bi(stream.unchecked_into())
    }

    async fn open_uni(&self) -> anyhow::Result<Self::SendStream> {
        let stream = JsFuture::from(self.transport.create_unidirectional_stream())
            .await
            .map_err(js_error)?;
        BrowserSendStream::new(stream.unchecked_into())
    }

    async fn accept_uni(&self) -> anyhow::Result<Self::ReceiveStream> {
        let stream = read_next(&self.incoming_uni_streams)
            .await
            .map_err(js_error)?
            .context("incoming unidirectional streams ended")?;
        BrowserReceiveStream::new(&stream.unchecked_into())
    }

    fn send_datagram(&self, bytes: BytesMut) -> anyhow::Result<()> {
        let writer = self
            .datagram_writer
            .as_ref()
            .context("this browser exposes no datagram writer")?;
        let write = writer.write_with_chunk(&Uint8Array::from(&bytes[..]));
        spawn_local(async move {
            if let Err(error) = JsFuture::from(write).await {
                tracing::warn!(?error, "failed to send datagram");
            }
        });
        Ok(())
    }

    async fn receive_datagram(&self) -> anyhow::Result<BytesMut> {
        let datagram = read_next(&self.datagram_reader)
            .await
            .map_err(js_error)?
            .context("datagram stream ended")?;
        Ok(BytesMut::from(&Uint8Array::from(datagram).to_vec()[..]))
    }

    fn stats(&self) -> TransportStats {
        TransportStats::default()
    }
}

/// The WebTransport spec replaced `datagrams.writable` with
/// `datagrams.createWritable()`; Safari only has the new form, Chrome has
/// both. Without either, datagrams cannot be sent but the session still works.
fn datagram_writable(datagrams: &WebTransportDatagramDuplexStream) -> Option<WritableStream> {
    let create_writable =
        js_sys::Reflect::get(datagrams, &JsValue::from_str("createWritable")).ok()?;
    if let Ok(create_writable) = create_writable.dyn_into::<js_sys::Function>() {
        return create_writable
            .call0(datagrams)
            .ok()?
            .dyn_into::<WritableStream>()
            .ok();
    }
    js_sys::Reflect::get(datagrams, &JsValue::from_str("writable"))
        .ok()?
        .dyn_into::<WritableStream>()
        .ok()
}

use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::transport_send_stream::TransportSendError;
use crate::modules::transport::{
    quic::quic_send_stream::QUICSendStream, transport_send_stream::TransportSendStream,
    webtransport::wt_send_stream::WtSendStream,
};

#[derive(Debug)]
pub enum DualSendStream {
    Quic(QUICSendStream),
    WebTransport(WtSendStream),
}

#[async_trait]
impl TransportSendStream for DualSendStream {
    async fn send(&mut self, buffer: &BytesMut) -> Result<(), TransportSendError> {
        match self {
            DualSendStream::Quic(s) => s.send(buffer).await,
            DualSendStream::WebTransport(s) => s.send(buffer).await,
        }
    }

    async fn close(&mut self) -> Result<(), TransportSendError> {
        match self {
            DualSendStream::Quic(s) => s.close().await,
            DualSendStream::WebTransport(s) => s.close().await,
        }
    }

    async fn reset(&mut self, error_code: u64) -> Result<(), TransportSendError> {
        match self {
            DualSendStream::Quic(s) => s.reset(error_code).await,
            DualSendStream::WebTransport(s) => s.reset(error_code).await,
        }
    }
}

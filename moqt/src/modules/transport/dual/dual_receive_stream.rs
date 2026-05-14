use std::task::Poll;

use async_trait::async_trait;
use bytes::BytesMut;

use crate::modules::transport::{
    quic::quic_receive_stream::QUICReceiveStream, read_error::ReadError,
    transport_receive_stream::TransportReceiveStream,
    webtransport::wt_receive_stream::WtReceiveStream,
};

#[derive(Debug)]
pub enum DualReceiveStream {
    Quic(QUICReceiveStream),
    WebTransport(WtReceiveStream),
}

#[async_trait]
impl TransportReceiveStream for DualReceiveStream {
    fn poll_read(
        &mut self,
        cx: &mut std::task::Context<'_>,
        buf: &mut BytesMut,
    ) -> Poll<Result<usize, ReadError>> {
        match self {
            DualReceiveStream::Quic(s) => s.poll_read(cx, buf),
            DualReceiveStream::WebTransport(s) => s.poll_read(cx, buf),
        }
    }
}

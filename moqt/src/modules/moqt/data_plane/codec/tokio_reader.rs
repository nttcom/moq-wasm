use std::pin::Pin;

use bytes::BytesMut;
use tokio::io::AsyncRead;

use crate::{
    TransportProtocol, modules::transport::transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub(crate) struct Reader<T: TransportProtocol, const N: usize> {
    receive_stream: T::ReceiveStream,
}

impl<T: TransportProtocol, const N: usize> Reader<T, N> {
    pub(crate) fn new(receive_stream: T::ReceiveStream) -> Self {
        Self { receive_stream }
    }
}

impl<T: TransportProtocol, const N: usize> AsyncRead for Reader<T, N> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut bytes_mut = BytesMut::with_capacity(N);
        bytes_mut.resize(N, 0);
        // TODO: handle the case where the received message is larger than the buffer size.
        Pin::new(&mut self.receive_stream)
            .poll_read(cx, &mut bytes_mut)
            .map(|result| {
                result.map(|size| {
                    buf.put_slice(&bytes_mut[..size]);
                })
            })
            .map_err(|e| std::io::Error::other(format!("read error: {:?}", e)))
    }
}

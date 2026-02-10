use anyhow::bail;
use bytes::BytesMut;

use crate::modules::{
    moqt::protocol::TransportProtocol, transport::transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub struct StreamReceiver<T: TransportProtocol> {
    pub(crate) receive_stream: T::ReceiveStream,
}

impl<T: TransportProtocol> StreamReceiver<T> {
    const RECEIVE_BYTES_CAPACITY: usize = 1024;

    pub async fn receive(&mut self) -> anyhow::Result<BytesMut> {
        let mut total_message = BytesMut::new();
        loop {
            let mut bytes = BytesMut::with_capacity(Self::RECEIVE_BYTES_CAPACITY);
            bytes.resize(Self::RECEIVE_BYTES_CAPACITY, 0);
            let message = match self.receive_stream.receive(&mut bytes).await {
                Ok(size) => size,
                Err(e) => {
                    tracing::error!("failed to receive message: {:?}", e);
                    bail!("failed to receive message: {:?}", e)
                },
            };
            if let Some(size) = message {
                tracing::debug!("Size {} message received", size);
                total_message.extend_from_slice(&bytes[..size]);
                if size == Self::RECEIVE_BYTES_CAPACITY {
                    tracing::debug!("Retry...");
                    tokio::task::yield_now().await;
                } else {
                    tracing::debug!("message length: {}", total_message.len());
                    return Ok(total_message);
                }
            } else {
                // TODO: make error handling more clear.
                bail!("Stream closed.")
            }
        }
    }
}

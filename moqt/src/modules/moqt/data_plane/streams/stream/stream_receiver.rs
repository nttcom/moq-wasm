use anyhow::bail;

use crate::modules::{
    moqt::protocol::TransportProtocol, transport::transport_receive_stream::TransportReceiveStream,
};

#[derive(Debug)]
pub struct StreamReceiver<T: TransportProtocol> {
    pub(crate) receive_stream: T::ReceiveStream,
}

impl<T: TransportProtocol> StreamReceiver<T> {
    const RECEIVE_BYTES_CAPACITY: usize = 1024;

    pub async fn receive(&mut self) -> anyhow::Result<Vec<u8>> {
        let mut total_message = vec![];
        loop {
            let mut bytes = vec![0u8; Self::RECEIVE_BYTES_CAPACITY];
            let message = self.receive_stream.receive(&mut bytes).await;
            if let Err(e) = message {
                tracing::error!("failed to receive message: {:?}", e);
                bail!("failed to receive message: {:?}", e)
            }
            if let Some(size) = message.unwrap() {
                tracing::debug!("Size {} message has been received", size);
                total_message.extend_from_slice(&bytes[..size]);
                if size == Self::RECEIVE_BYTES_CAPACITY {
                    tracing::debug!("Retry...");
                    tokio::task::yield_now().await;
                } else {
                    tracing::debug!("message length: {}", total_message.len());
                    return Ok(total_message);
                }
            } else {
                bail!("BiStream closed.")
            }
        }
    }
}

use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};

use crate::modules::moqt::{
    data_plane::codec::{
        control_message_decoder::ControlMessageDecoder, subgroup_decoder::SubgroupDecoder,
        tokio_reader::Reader,
    },
    protocol::TransportProtocol,
};

pub(crate) type BiStreamReceiver<T> = StreamReceiver<T, 1024, ControlMessageDecoder>;
pub(crate) type UniStreamReceiver<T> = StreamReceiver<T, { 64 * 1024 }, SubgroupDecoder>;

#[derive(Debug)]
pub struct StreamReceiver<T: TransportProtocol, const U: usize, D: Decoder> {
    framed_read: FramedRead<Reader<T, U>, D>,
}

impl<T: TransportProtocol, const U: usize, D: Decoder> StreamReceiver<T, U, D> {
    pub(crate) fn new(receive_stream: T::ReceiveStream, decoder: D) -> Self {
        let inner = Reader::<T, U>::new(receive_stream);
        let framed_read = FramedRead::with_capacity(inner, decoder, U);
        Self { framed_read }
    }

    pub async fn receive(&mut self) -> Option<anyhow::Result<D::Item>> {
        let item = self.framed_read.next().await;
        match item {
            Some(Ok(item)) => Some(Ok(item)),
            Some(Err(_)) => Some(Err(anyhow::anyhow!("Failed to decode message"))),
            None => {
                tracing::debug!("Stream has ended");
                None
            }
        }
    }
}

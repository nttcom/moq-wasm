use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};

use crate::modules::moqt::{
    data_plane::codec::{
        data_frame_decoder::DataFrameDecoder, message_decoder::MessageDecoder, tokio_reader::Reader,
    },
    protocol::TransportProtocol,
};

pub(crate) type BiStreamReceiver<T> = StreamReceiver<T, 1024, MessageDecoder>;
pub(crate) type UniStreamReceiver<T> = StreamReceiver<T, { 64 * 1024 }, DataFrameDecoder>;

#[derive(Debug)]
pub struct StreamReceiver<T: TransportProtocol, const U: usize, D: Decoder> {
    framed_read: FramedRead<Reader<T, U>, D>,
}

impl<T: TransportProtocol, const U: usize, D: Decoder> StreamReceiver<T, U, D> {
    pub(crate) fn new(receive_stream: T::ReceiveStream, decoder: D) -> Self {
        let inner = Reader::<T, U>::new(receive_stream);
        let framed_read = FramedRead::new(inner, decoder);
        Self { framed_read }
    }

    pub async fn receive(&mut self) -> Option<anyhow::Result<D::Item>> {
        let item = self.framed_read.next().await;
        match item {
            Some(Ok(item)) => Some(Ok(item)),
            Some(Err(_)) => Some(Err(anyhow::anyhow!("Failed to decode message"))),
            // The stream has ended.
            None => {
                tracing::debug!("Stream has ended");
                None
            }
        }
    }
}

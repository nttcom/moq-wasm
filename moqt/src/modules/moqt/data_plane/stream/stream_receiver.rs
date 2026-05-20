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
pub(crate) enum StreamReceiveError {
    Closed(String),
    Decode(String),
}

impl std::fmt::Display for StreamReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed(error) => write!(f, "stream closed: {error}"),
            Self::Decode(error) => write!(f, "failed to decode stream data: {error}"),
        }
    }
}

impl std::error::Error for StreamReceiveError {}

#[derive(Debug)]
pub struct StreamReceiver<T: TransportProtocol, const U: usize, D: Decoder> {
    framed_read: FramedRead<Reader<T, U>, D>,
}

impl<T: TransportProtocol, const U: usize, D: Decoder> StreamReceiver<T, U, D> {
    pub(crate) fn new(receive_stream: T::ReceiveStream, decoder: D) -> Self {
        let inner = Reader::<T, U>::new(receive_stream);
        // let framed_read = FramedRead::new(inner, decoder);
        let framed_read = FramedRead::with_capacity(inner, decoder, U);
        Self { framed_read }
    }

    pub async fn receive(&mut self) -> Result<Option<D::Item>, StreamReceiveError>
    where
        D::Error: std::fmt::Debug,
    {
        let item = self.framed_read.next().await;
        match item {
            Some(Ok(item)) => Ok(Some(item)),
            Some(Err(error)) => {
                let error = format!("{error:?}");
                if error.contains("read error: Closed") {
                    Err(StreamReceiveError::Closed(error))
                } else {
                    Err(StreamReceiveError::Decode(error))
                }
            }
            // The stream has ended.
            None => {
                tracing::debug!("Stream has ended");
                Ok(None)
            }
        }
    }
}

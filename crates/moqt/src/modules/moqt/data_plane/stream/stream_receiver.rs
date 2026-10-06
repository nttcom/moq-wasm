use crate::modules::transport::transport_connection::BoxedReceiveStream;
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};

use crate::modules::moqt::data_plane::codec::{
    control_message_decoder::ControlMessageDecoder, tokio_reader::Reader,
    uni_stream_decoder::UniStreamDecoder,
};

pub(crate) type BiStreamReceiver = StreamReceiver<1024, ControlMessageDecoder>;
pub(crate) type UniStreamReceiver = StreamReceiver<{ 64 * 1024 }, UniStreamDecoder>;

#[derive(Debug)]
pub enum StreamReceiveError {
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
pub struct StreamReceiver<const U: usize, D: Decoder> {
    framed_read: FramedRead<Reader, D>,
}

impl<const U: usize, D: Decoder> StreamReceiver<U, D> {
    pub(crate) fn new(receive_stream: BoxedReceiveStream, decoder: D) -> Self {
        let inner = Reader::new(receive_stream);
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

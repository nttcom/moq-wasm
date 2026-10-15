use crate::modules::transport::{read_error::ReadError, transport_connection::BoxedReceiveStream};
use thiserror::Error;
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};

use crate::modules::moqt::data_plane::codec::{
    control_message_decoder::ControlMessageDecoder, tokio_reader::Reader,
    uni_stream_decoder::UniStreamDecoder,
};

pub(crate) type BiStreamReceiver = StreamReceiver<1024, ControlMessageDecoder>;
pub(crate) type UniStreamReceiver = StreamReceiver<{ 64 * 1024 }, UniStreamDecoder>;

#[derive(Debug, Error)]
pub enum StreamReceiveError {
    #[error("stream closed: {0}")]
    Closed(ReadError),
    #[error("failed to decode stream data: {0}")]
    Decode(String),
    /// The stream decoded but violates the protocol, e.g. an unknown Object
    /// Status (draft-14 §10.2.1.1). The caller should close the session.
    #[error("protocol violation: {0}")]
    ProtocolViolation(String),
}

#[derive(Debug)]
pub struct StreamReceiver<const U: usize, D: Decoder> {
    framed_read: FramedRead<Reader, D>,
}

impl<const U: usize, D: Decoder<Error = std::io::Error>> StreamReceiver<U, D> {
    pub(crate) fn new(receive_stream: BoxedReceiveStream, decoder: D) -> Self {
        let inner = Reader::new(receive_stream);
        let framed_read = FramedRead::with_capacity(inner, decoder, U);
        Self { framed_read }
    }

    pub async fn receive(&mut self) -> Result<Option<D::Item>, StreamReceiveError> {
        let item = self.framed_read.next().await;
        match item {
            Some(Ok(item)) => Ok(Some(item)),
            Some(Err(error)) => match transport_error(&error) {
                Some(read_error) => Err(StreamReceiveError::Closed(read_error.clone())),
                None => Err(StreamReceiveError::Decode(error.to_string())),
            },
            // The stream has ended.
            None => {
                tracing::debug!("Stream has ended");
                Ok(None)
            }
        }
    }
}

fn transport_error(error: &std::io::Error) -> Option<&ReadError> {
    error.get_ref()?.downcast_ref()
}

#[cfg(test)]
mod tests {
    use std::task::Poll;

    use super::{StreamReceiveError, UniStreamReceiver};
    use crate::modules::{
        moqt::data_plane::codec::uni_stream_decoder::UniStreamDecoder,
        transport::{read_error::ReadError, transport_receive_stream::MockTransportReceiveStream},
    };

    fn receiver_reading(result: Result<&'static [u8], ReadError>) -> UniStreamReceiver {
        let mut receive_stream = MockTransportReceiveStream::new();
        receive_stream
            .expect_poll_read()
            .returning(move |_, buf| Poll::Ready(result.clone().map(|bytes| buf.put_slice(bytes))));
        UniStreamReceiver::new(Box::new(receive_stream), UniStreamDecoder::new())
    }

    #[tokio::test]
    async fn connection_loss_is_reported_as_closed() {
        // Arrange
        let mut receiver = receiver_reading(Err(ReadError::ConnectionLost));

        // Act
        let result = receiver.receive().await;

        // Assert
        assert!(matches!(
            result,
            Err(StreamReceiveError::Closed(ReadError::ConnectionLost))
        ));
    }

    #[tokio::test]
    async fn stream_reset_is_reported_as_closed() {
        // Arrange
        let mut receiver = receiver_reading(Err(ReadError::Reset));

        // Act
        let result = receiver.receive().await;

        // Assert
        assert!(matches!(
            result,
            Err(StreamReceiveError::Closed(ReadError::Reset))
        ));
    }

    #[tokio::test]
    async fn unknown_stream_type_is_reported_as_decode_error() {
        // Arrange
        let mut receiver = receiver_reading(Ok(&[0x01]));

        // Act
        let result = receiver.receive().await;

        // Assert
        assert!(matches!(result, Err(StreamReceiveError::Decode(_))));
    }
}

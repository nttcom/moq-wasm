//! Framed control messages on the draft-18 control streams and request
//! streams (§10).

use bytes::BytesMut;
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, FramedRead};

use super::{
    codec::DecodeError,
    messages::{Message, decode_message, take_message},
};
use crate::modules::{
    moqt::data_plane::codec::tokio_reader::Reader,
    transport::{
        read_error::ReadError,
        transport_connection::{BoxedReceiveStream, BoxedSendStream},
        transport_send_stream::TransportSendError,
    },
};

#[derive(Debug, Default)]
struct MessageDecoder;

impl Decoder for MessageDecoder {
    type Item = Message;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let invalid = |error: DecodeError| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())
        };
        match take_message(src) {
            Ok((message_type, payload)) => decode_message(message_type, payload)
                .map(Some)
                .map_err(invalid),
            Err(DecodeError::NeedMoreData) => Ok(None),
            Err(violation) => Err(invalid(violation)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MessageReceiveError {
    #[error("the session ended")]
    SessionEnded,
    #[error("the stream was reset")]
    Reset,
    #[error("malformed message: {0}")]
    Malformed(String),
}

#[derive(Debug)]
pub(crate) struct MessageReceiver {
    framed_read: FramedRead<Reader, MessageDecoder>,
}

impl MessageReceiver {
    pub(crate) fn new(receive_stream: BoxedReceiveStream) -> Self {
        Self {
            framed_read: FramedRead::with_capacity(
                Reader::new(receive_stream),
                MessageDecoder,
                1024,
            ),
        }
    }

    /// `Ok(None)` once the peer has finished the stream.
    pub(crate) async fn receive(&mut self) -> Result<Option<Message>, MessageReceiveError> {
        let Some(received) = self.framed_read.next().await else {
            return Ok(None);
        };
        received.map(Some).map_err(|error| {
            match error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<ReadError>())
            {
                Some(ReadError::ConnectionLost) => MessageReceiveError::SessionEnded,
                Some(ReadError::Closed | ReadError::Reset) => MessageReceiveError::Reset,
                None => MessageReceiveError::Malformed(error.to_string()),
            }
        })
    }
}

#[derive(Debug)]
pub(crate) struct MessageSender {
    stream: tokio::sync::Mutex<BoxedSendStream>,
}

impl MessageSender {
    pub(crate) fn new(stream: BoxedSendStream) -> Self {
        Self {
            stream: tokio::sync::Mutex::new(stream),
        }
    }

    pub(crate) async fn send(&self, message: &BytesMut) -> Result<(), TransportSendError> {
        self.stream.lock().await.send(message).await
    }

    pub(crate) async fn close(&self) -> Result<(), TransportSendError> {
        self.stream.lock().await.close().await
    }

    pub(crate) async fn reset(&self, code: u64) -> Result<(), TransportSendError> {
        self.stream.lock().await.reset(code).await
    }
}

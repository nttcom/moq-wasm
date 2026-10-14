//! Framed control messages on the draft-18 control streams and request
//! streams (§10).

use bytes::BytesMut;
use tokio_util::codec::Decoder;

use super::{
    codec::DecodeError,
    messages::{Message, decode_message, take_message},
};
use crate::modules::{
    moqt::data_plane::stream::stream_receiver::StreamReceiver,
    transport::{transport_connection::BoxedSendStream, transport_send_stream::TransportSendError},
};

#[derive(Debug, Default)]
pub(crate) struct MessageDecoder;

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

pub(crate) type MessageReceiver = StreamReceiver<1024, MessageDecoder>;

/// The sending half of a control or request stream.
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

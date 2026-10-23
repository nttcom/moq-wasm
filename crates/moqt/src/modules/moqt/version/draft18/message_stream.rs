//! Framed control messages on the draft-18 control streams and request
//! streams (§10).

use bytes::BytesMut;
use tokio_util::codec::Decoder;

use super::{
    codec::DecodeError,
    messages::{Message, decode_message, take_message},
};
use crate::modules::moqt::data_plane::stream::stream_receiver::StreamReceiver;

pub(crate) type MessageReceiver = StreamReceiver<1024, MessageDecoder>;

#[derive(Debug, Default)]
pub(crate) struct MessageDecoder;

impl Decoder for MessageDecoder {
    type Item = Message;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match take_message(src) {
            Ok((message_type, payload)) => Ok(Some(decode_message(message_type, payload)?)),
            Err(DecodeError::NeedMoreData) => Ok(None),
            Err(violation) => Err(violation.into()),
        }
    }
}

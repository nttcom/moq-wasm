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

#[cfg(test)]
mod tests {
    use tokio_util::codec::Decoder;

    use super::MessageDecoder;
    use crate::modules::{
        moqt::version::draft18::messages::{SETUP, frame},
        test_support::wire_noise,
    };

    #[test]
    fn a_framed_message_of_every_type_decodes_or_is_refused() {
        // SETUP is the one type above a single byte.
        for message_type in (0..=0x7f).chain([SETUP]) {
            for payload in wire_noise() {
                // Arrange
                let mut frame = frame(message_type, &payload).unwrap();

                // Act
                let decoded = MessageDecoder.decode(&mut frame);

                // Assert: a malformed payload does not wait for more bytes
                assert!(
                    matches!(decoded, Ok(Some(_))) && frame.is_empty() || decoded.is_err(),
                    "{message_type:#x} {payload:02x?}: {decoded:?}, {} bytes left",
                    frame.len()
                );
            }
        }
    }
}

use bytes::BytesMut;

use crate::modules::moqt::messages::moqt_message_error::MOQTMessageError;

// TODO: All message conform MOQTMessage, then rename `MOQTPayload`.
pub trait MOQTMessage: Send + Sync {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError>
    where
        Self: Sized;
    fn packetize(&self) -> BytesMut;
}

use bytes::BytesMut;
use std::any::Any;

use crate::modules::moqt::messages::moqt_message_error::MOQTMessageError;

// TODO: All message conform MOQTMessage, then rename `MOQTPayload`.
pub trait MOQTMessage: Send + Sync + Any {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError>
    where
        Self: Sized;
    fn packetize(&self) -> BytesMut;
}

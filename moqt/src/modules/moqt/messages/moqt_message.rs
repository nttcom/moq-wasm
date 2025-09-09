use bytes::BytesMut;
use std::any::Any;

use crate::modules::moqt::messages::moqt_message_error::MOQTMessageError;

pub trait MOQTMessage: Send + Sync {
    fn depacketize(buf: &mut BytesMut) -> Result<Self, MOQTMessageError>
    where
        Self: Sized;
    fn packetize(&self) -> BytesMut;
    // Method to enable downcasting from MOQTPayload to Message
    fn as_any(&self) -> &dyn Any;
}

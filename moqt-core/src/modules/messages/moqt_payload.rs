use anyhow::Result;
use bytes::BytesMut;
use std::any::Any;

// Each message implements this trait
pub trait MOQTPayload: Send + Sync {
    // If any invalid data is received, return Err as Result type
    fn depacketize(buf: &mut BytesMut) -> Result<Self>
    where
        Self: Sized;
    // Write the data to be sent into the buffer. Note that it does not return the written buffer.
    fn packetize(&self, buf: &mut BytesMut);
    // Method to enable downcasting from MOQTPayload to Message
    fn as_any(&self) -> &dyn Any;
}

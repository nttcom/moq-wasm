use anyhow::Result;
use bytes::BytesMut;
use std::any::Any;

pub trait MOQTPayload: Send + Sync {
    fn depacketize(buf: &mut BytesMut) -> Result<Self>
    where
        Self: Sized;
    fn packetize(&self, buf: &mut BytesMut);
    // Method to enable downcasting from MOQTPayload to Message
    fn as_any(&self) -> &dyn Any;
}

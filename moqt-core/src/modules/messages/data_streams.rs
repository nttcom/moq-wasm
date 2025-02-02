pub mod datagram;
pub mod object_status;
pub mod stream_per_subgroup;
pub mod stream_per_track;

use anyhow::Result;
use bytes::BytesMut;
use std::any::Any;

pub trait DataStreams: Send + Sync {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized;
    fn packetize(&self, buf: &mut BytesMut);
    // Method to enable downcasting from MOQTPayload to Message
    fn as_any(&self) -> &dyn Any;
}

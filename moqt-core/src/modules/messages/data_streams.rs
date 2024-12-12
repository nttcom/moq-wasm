pub mod object_datagram;
pub mod object_status;
pub mod object_stream_subgroup;
pub mod object_stream_track;
pub mod stream_header_subgroup;
pub mod stream_header_track;

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

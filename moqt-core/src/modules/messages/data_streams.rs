pub mod datagram;
pub mod object_status;
pub mod subgroup_stream;
pub mod track_stream;

use anyhow::Result;
use bytes::BytesMut;

pub trait DataStreams: Send + Sync {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized;
    fn packetize(&self, buf: &mut BytesMut);
}

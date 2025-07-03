pub mod datagram;
pub mod datagram_status;
pub mod extension_header;
pub mod object_status;
pub mod subgroup_stream;

use anyhow::Result;
use bytes::BytesMut;

pub trait DataStreams: Send + Sync {
    fn depacketize(read_cur: &mut std::io::Cursor<&[u8]>) -> Result<Self>
    where
        Self: Sized;
    fn packetize(&self, buf: &mut BytesMut);
}

#[derive(Debug, PartialEq, Clone)]
pub enum DatagramObject {
    ObjectDatagram(datagram::Object),
    ObjectDatagramStatus(datagram_status::Object),
}

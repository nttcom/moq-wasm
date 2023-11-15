use anyhow::Result;
use bytes::BytesMut;

// TODO: MOQTPayloadにする
pub(crate) trait Payload {
    fn depacketize(buf: &mut BytesMut) -> Result<Self>
    where
        Self: Sized;
    fn packetize(&self, buf: &mut BytesMut);
}

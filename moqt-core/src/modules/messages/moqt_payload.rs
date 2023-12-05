use anyhow::Result;
use bytes::BytesMut;

pub trait MOQTPayload {
    fn depacketize(buf: &mut BytesMut) -> Result<Self>
    where
        Self: Sized;
    fn packetize(&self, buf: &mut BytesMut);
}

use anyhow::Result;
use bytes::BytesMut;

// 各messageがこれを実装する
pub trait MOQTPayload {
    // 何らかの不正なデータが送られてきた場合は、Errを返す
    fn depacketize(buf: &mut BytesMut) -> Result<Self>
    where
        Self: Sized;
    fn packetize(&self, buf: &mut BytesMut);
}

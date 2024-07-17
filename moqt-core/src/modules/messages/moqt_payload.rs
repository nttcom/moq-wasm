use anyhow::Result;
use bytes::BytesMut;
use std::any::Any;

// 各messageがこれを実装する
pub trait MOQTPayload: Send + Sync {
    /// 何らかの不正なデータが送られてきた場合は、Errを返すのでResult型
    fn depacketize(buf: &mut BytesMut) -> Result<Self>
    where
        Self: Sized;
    /// 送信するデータをbufferに書き込む。書き込んだbufferを返すわけではないので注意
    fn packetize(&self, buf: &mut BytesMut);
    /// MOQTPayloadからMessageへのダウンキャストを可能にするためのメソッド
    fn as_any(&self) -> &dyn Any;
}

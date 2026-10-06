use std::fmt::Debug;

use crate::modules::{
    executor::{MaybeSend, MaybeSync},
    transport::{
        transport_addresses::TransportAddresses, transport_receive_stream::TransportReceiveStream,
        transport_send_stream::TransportSendStream, transport_stats::TransportStats,
    },
};
use async_trait::async_trait;

/// How the transport ended: `code` is the peer's termination code when the
/// connection was closed by the application layer (draft-14 §3.4), `None` for
/// transport-level failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransportClose {
    pub(crate) code: Option<u32>,
    pub(crate) reason: String,
}

pub(crate) type BoxedSendStream = Box<dyn TransportSendStream>;
pub(crate) type BoxedReceiveStream = Box<dyn TransportReceiveStream>;
pub(crate) type BoxedConnection = Box<dyn TransportConnection>;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub(crate) trait TransportConnection: MaybeSend + MaybeSync + Debug {
    async fn closed(&self) -> TransportClose;
    fn close(&self, code: u32, reason: &str);
    async fn open_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)>;
    async fn accept_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)>;
    async fn open_uni(&self) -> anyhow::Result<BoxedSendStream>;
    async fn accept_uni(&self) -> anyhow::Result<BoxedReceiveStream>;
    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<()>;
    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut>;
    fn stats(&self) -> TransportStats;
    fn addresses(&self) -> TransportAddresses;
}

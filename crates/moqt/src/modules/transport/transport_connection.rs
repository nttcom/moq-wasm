use std::fmt::{self, Debug};

use crate::modules::{
    executor::{MaybeSend, MaybeSync},
    transport::{
        application_protocol::NegotiatedProtocol, transport_addresses::TransportAddresses,
        transport_receive_stream::TransportReceiveStream,
        transport_send_stream::TransportSendStream, transport_stats::TransportStats,
    },
};
use async_trait::async_trait;

/// What became of an object `DatagramSender::send_object` sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DatagramDelivery {
    /// A connection took the datagram.
    Sent,
    /// The datagram exceeds the maximum datagram size of every connection,
    /// so it was dropped without notice (draft-14 §10.3, draft-18 §11.3).
    TooLarge,
}

/// How the transport ended: `code` is the peer's termination code when the
/// connection was closed by the application layer (draft-14 §3.4), `None` for
/// transport-level failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportClose {
    pub code: Option<u32>,
    pub reason: String,
}

impl fmt::Display for TransportClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(f, "code {code}: {}", self.reason),
            None => f.write_str(&self.reason),
        }
    }
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
    /// `open_bi` without waiting for the peer to raise its stream limit:
    /// `None` when the limit allows no stream now.
    async fn try_open_bi(&self) -> anyhow::Result<Option<(BoxedSendStream, BoxedReceiveStream)>>;
    async fn accept_bi(&self) -> anyhow::Result<(BoxedSendStream, BoxedReceiveStream)>;
    async fn open_uni(&self) -> anyhow::Result<BoxedSendStream>;
    async fn accept_uni(&self) -> anyhow::Result<BoxedReceiveStream>;
    fn send_datagram(&self, bytes: bytes::BytesMut) -> anyhow::Result<DatagramDelivery>;
    async fn receive_datagram(&self) -> anyhow::Result<bytes::BytesMut>;
    fn stats(&self) -> TransportStats;
    fn addresses(&self) -> TransportAddresses;
    fn negotiated_protocol(&self) -> NegotiatedProtocol;
}

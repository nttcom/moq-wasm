use crate::modules::{
    executor::{BoxFuture, MaybeSend, MaybeSync},
    moqt::domains::endpoint::CertificateVerification,
    transport::{
        accept_error::AcceptError, application_protocol::ApplicationProtocols,
        connect_target::ConnectTarget, transport_connection::BoxedConnection,
    },
};
use async_trait::async_trait;
use std::time::Duration;

/// The rest of one accepted connection attempt's transport handshake (on
/// `NATIVE`: QUIC, then the WebTransport CONNECT on `h3`).
pub(crate) type TransportAccepting = BoxFuture<anyhow::Result<BoxedConnection>>;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub(crate) trait TransportConnectionCreator: MaybeSend + MaybeSync + 'static {
    /// The head start `moqt://` or `https://` gets before `wss://` is dialed
    /// alongside it; `None` for a client that cannot open `wss://`.
    const WEB_SOCKET_HEAD_START: Option<Duration> = None;

    fn client(
        port_num: u16,
        verification: &CertificateVerification,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        keep_alive_sec: u64,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    async fn create_new_transport(&self, target: &ConnectTarget)
    -> anyhow::Result<BoxedConnection>;
    /// Must not wait on the peer: every wait the peer controls belongs in the
    /// returned `TransportAccepting`, or one stalled peer would hold back every
    /// connection after it.
    async fn accept_new_transport(&self) -> Result<TransportAccepting, AcceptError>;
    /// From now on every connection attempt is refused at the transport
    /// level while `accept_new_transport` is polled; established
    /// connections are unaffected.
    fn refuse_new_connections(&self);
    async fn wait_idle(&self);
    fn local_port(&self) -> anyhow::Result<u16>;
}

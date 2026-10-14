use crate::modules::{
    executor::{MaybeSend, MaybeSync},
    transport::{
        application_protocol::ApplicationProtocols, connect_target::ConnectTarget,
        transport_connection::BoxedConnection,
    },
};
use async_trait::async_trait;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub(crate) trait TransportConnectionCreator: MaybeSend + MaybeSync + 'static {
    fn client(
        port_num: u16,
        verify_certificate: bool,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn client_with_custom_cert(
        port_num: u16,
        custom_cert_path: &str,
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
    async fn accept_new_transport(&self) -> anyhow::Result<BoxedConnection>;
    /// From now on every connection attempt is refused at the transport
    /// level while `accept_new_transport` is polled; established
    /// connections are unaffected.
    fn refuse_new_connections(&self);
    async fn wait_idle(&self);
    fn local_port(&self) -> anyhow::Result<u16>;
}

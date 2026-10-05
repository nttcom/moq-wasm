use crate::modules::{
    executor::{MaybeSend, MaybeSync},
    transport::{connect_target::ConnectTarget, transport_connection::TransportConnection},
};
use async_trait::async_trait;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub(crate) trait TransportConnectionCreator: MaybeSend + MaybeSync + 'static {
    type Connection: TransportConnection;

    fn client(port_num: u16, verify_certificate: bool) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn client_with_custom_cert(port_num: u16, custom_cert_path: &str) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn server(
        cert_path: &str,
        key_path: &str,
        port_num: u16,
        keep_alive_sec: u64,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<Self::Connection>;
    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection>;
}

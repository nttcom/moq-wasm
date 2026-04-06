use std::net::SocketAddr;

use crate::modules::transport::transport_connection::TransportConnection;
use async_trait::async_trait;

#[async_trait]
pub(crate) trait TransportConnectionCreator: Send + Sync + 'static {
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
        remote_address: SocketAddr,
        host: &str,
    ) -> anyhow::Result<Self::Connection>;
    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection>;
}

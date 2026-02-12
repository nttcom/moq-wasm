use std::net::SocketAddr;

use async_trait::async_trait;

use crate::modules::transport::transport_connection_creator::TransportConnectionCreator;
use super::wt_connection::WtConnection;

pub struct WtConnectionCreator;

#[async_trait]
impl TransportConnectionCreator for WtConnectionCreator {
    type Connection = WtConnection;

    fn client(_port_num: u16, _verify_certificate: bool) -> anyhow::Result<Self> {
        todo!("WebTransport client not yet implemented")
    }

    fn client_with_custom_cert(_port_num: u16, _custom_cert_path: &str) -> anyhow::Result<Self> {
        todo!("WebTransport client_with_custom_cert not yet implemented")
    }

    fn server(
        _cert_path: String,
        _key_path: String,
        _port_num: u16,
        _keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        todo!("WebTransport server not yet implemented")
    }

    async fn create_new_transport(
        &self,
        _remote_address: SocketAddr,
        _host: &str,
    ) -> anyhow::Result<Self::Connection> {
        todo!("WebTransport create_new_transport not yet implemented")
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection> {
        todo!("WebTransport accept_new_transport not yet implemented")
    }
}

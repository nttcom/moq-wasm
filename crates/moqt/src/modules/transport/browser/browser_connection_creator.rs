use async_trait::async_trait;
use wasm_bindgen_futures::JsFuture;
use web_sys::WebTransport;

use super::{browser_connection::BrowserConnection, js_value::js_error};
use crate::modules::transport::{
    connect_target::{ClientTransport, ConnectTarget},
    transport_connection_creator::TransportConnectionCreator,
};

pub struct BrowserConnectionCreator;

#[async_trait(?Send)]
impl TransportConnectionCreator for BrowserConnectionCreator {
    type Connection = BrowserConnection;

    fn client(_port_num: u16, verify_certificate: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(
            verify_certificate,
            "the browser always verifies the server certificate"
        );
        Ok(Self)
    }

    fn client_with_custom_cert(_port_num: u16, _custom_cert_path: &str) -> anyhow::Result<Self> {
        anyhow::bail!(
            "the browser cannot load a certificate file; trust the certificate in the browser instead"
        )
    }

    fn server(
        _cert_path: &str,
        _key_path: &str,
        _port_num: u16,
        _keep_alive_sec: u64,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("the browser cannot accept connections")
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<Self::Connection> {
        if target.transport != ClientTransport::WebTransport {
            anyhow::bail!(
                "browser endpoint requires an https:// url, got {}",
                target.url
            );
        }
        let transport = WebTransport::new(target.url.as_str()).map_err(js_error)?;
        JsFuture::from(transport.ready()).await.map_err(js_error)?;
        BrowserConnection::new(transport)
    }

    async fn accept_new_transport(&mut self) -> anyhow::Result<Self::Connection> {
        anyhow::bail!("the browser cannot accept connections")
    }
}

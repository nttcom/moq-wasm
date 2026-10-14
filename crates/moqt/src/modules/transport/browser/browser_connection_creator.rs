use crate::modules::transport::transport_connection::BoxedConnection;
use async_trait::async_trait;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{WebTransport, WebTransportOptions};

use super::{browser_connection::BrowserConnection, js_value::js_error};
use crate::modules::transport::{
    application_protocol::ApplicationProtocols,
    connect_target::{ClientTransport, ConnectTarget},
    transport_connection_creator::TransportConnectionCreator,
};

pub struct BrowserConnectionCreator {
    protocols: ApplicationProtocols,
}

#[async_trait(?Send)]
impl TransportConnectionCreator for BrowserConnectionCreator {
    fn client(
        _port_num: u16,
        verify_certificate: bool,
        protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            verify_certificate,
            "the browser always verifies the server certificate"
        );
        Ok(Self { protocols })
    }

    fn client_with_custom_cert(
        _port_num: u16,
        _custom_cert_path: &str,
        _protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        anyhow::bail!(
            "the browser cannot load a certificate file; trust the certificate in the browser instead"
        )
    }

    fn server(
        _cert_path: &str,
        _key_path: &str,
        _port_num: u16,
        _keep_alive_sec: u64,
        _protocols: ApplicationProtocols,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("the browser cannot accept connections")
    }

    async fn create_new_transport(
        &self,
        target: &ConnectTarget,
    ) -> anyhow::Result<BoxedConnection> {
        if target.transport != ClientTransport::WebTransport {
            anyhow::bail!(
                "browser endpoint requires an https:// url, got {}",
                target.url
            );
        }
        let options = web_transport_options(self.protocols.web_transport)?;
        let transport =
            WebTransport::new_with_options(target.url.as_str(), &options).map_err(js_error)?;
        JsFuture::from(transport.ready()).await.map_err(js_error)?;
        Ok(Box::new(BrowserConnection::new(transport)?))
    }

    async fn accept_new_transport(&self) -> anyhow::Result<BoxedConnection> {
        anyhow::bail!("the browser cannot accept connections")
    }

    fn refuse_new_connections(&self) {}

    async fn wait_idle(&self) {}

    fn local_port(&self) -> anyhow::Result<u16> {
        anyhow::bail!("a browser endpoint has no local socket")
    }
}

/// Sets `WebTransportOptions.protocols`, which web-sys does not expose; the
/// browser sends it as WT-Available-Protocols.
fn web_transport_options(protocols: &[&str]) -> anyhow::Result<WebTransportOptions> {
    let options = WebTransportOptions::new();
    let protocols: js_sys::Array = protocols
        .iter()
        .map(|protocol| JsValue::from_str(protocol))
        .collect();
    js_sys::Reflect::set(&options, &JsValue::from_str("protocols"), &protocols)
        .map_err(js_error)?;
    Ok(options)
}

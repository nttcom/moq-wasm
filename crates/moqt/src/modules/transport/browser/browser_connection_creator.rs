use std::{pin::pin, time::Duration};

use crate::modules::{executor, transport::transport_connection::BoxedConnection};
use async_trait::async_trait;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{WebTransport, WebTransportOptions};

use super::{browser_connection::BrowserConnection, browser_web_socket, js_value::js_error};
use crate::modules::transport::{
    accept_error::AcceptError,
    application_protocol::ApplicationProtocols,
    connect_target::{ClientTransport, ConnectTarget},
    transport_connection_creator::TransportConnectionCreator,
};

/// Both the delay before a second WebTransport is dialed and WebTransport's
/// head start over WebSocket. A first dial whose initial packet is lost waits
/// about a second for QUIC to resend it, so a second dial started together
/// with WebSocket keeps that loss from handing the race to WebSocket.
const WEB_TRANSPORT_HEDGE: Duration = Duration::from_millis(500);

pub struct BrowserConnectionCreator {
    protocols: ApplicationProtocols,
}

#[async_trait(?Send)]
impl TransportConnectionCreator for BrowserConnectionCreator {
    const WEB_SOCKET_HEAD_START: Option<Duration> = Some(WEB_TRANSPORT_HEDGE);

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
        match target.transport {
            ClientTransport::WebTransport => {}
            ClientTransport::WebSocket => {
                let connection = browser_web_socket::connect(target, self.protocols).await?;
                return Ok(Box::new(connection));
            }
            ClientTransport::Quic => anyhow::bail!(
                "browser endpoint requires an https:// or wss:// url, got {}",
                target.url
            ),
        }
        let mut first = pin!(self.open_web_transport(&target.url));
        let transport = match executor::timeout(WEB_TRANSPORT_HEDGE, first.as_mut()).await {
            Ok(opened) => opened?,
            Err(()) => {
                tracing::debug!(url = %target.url, "dialing a second WebTransport");
                let mut second = pin!(self.open_web_transport(&target.url));
                tokio::select! {
                    biased;
                    opened = first.as_mut() => match opened {
                        Ok(transport) => transport,
                        Err(error) => second.await.map_err(|_| error)?,
                    },
                    opened = second.as_mut() => match opened {
                        Ok(transport) => transport,
                        Err(_) => first.await?,
                    },
                }
            }
        };
        Ok(Box::new(BrowserConnection::new(transport)?))
    }

    async fn accept_new_transport(&self) -> Result<BoxedConnection, AcceptError> {
        Err(anyhow::anyhow!("the browser cannot accept connections").into())
    }

    fn refuse_new_connections(&self) {}

    async fn wait_idle(&self) {}

    fn local_port(&self) -> anyhow::Result<u16> {
        anyhow::bail!("a browser endpoint has no local socket")
    }
}

impl BrowserConnectionCreator {
    async fn open_web_transport(&self, url: &str) -> anyhow::Result<WebTransport> {
        let options = web_transport_options(self.protocols.web_transport)?;
        // A browser without WebTransport throws here, so the fallback starts at once.
        let transport = WebTransport::new_with_options(url, &options).map_err(js_error)?;
        let pending = CloseUnlessReady(Some(transport));
        let transport = pending.0.as_ref().expect("set above");
        JsFuture::from(transport.ready()).await.map_err(js_error)?;
        Ok(pending.into_ready())
    }
}

/// Closes the WebTransport when dropped before `ready` resolves, as when it
/// loses the race; otherwise the browser keeps connecting and later holds a
/// session nobody reads.
struct CloseUnlessReady(Option<WebTransport>);

impl CloseUnlessReady {
    fn into_ready(mut self) -> WebTransport {
        self.0.take().expect("taken once")
    }
}

impl Drop for CloseUnlessReady {
    fn drop(&mut self) {
        if let Some(transport) = self.0.take() {
            transport.close();
        }
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

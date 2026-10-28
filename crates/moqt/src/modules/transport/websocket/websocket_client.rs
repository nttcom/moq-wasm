use std::sync::Arc;

use anyhow::Context;
use quinn::rustls::{self, pki_types::ServerName};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_tungstenite::{
    client_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderValue, header::SEC_WEBSOCKET_PROTOCOL},
    },
};

use super::{HANDSHAKE_TIMEOUT, tungstenite_transport::split, web_socket_config};
use crate::modules::transport::{
    application_protocol::ApplicationProtocols,
    connect_target::ConnectTarget,
    qmux::{qmux_connection::QmuxConnection, stream_id::Role, subprotocol},
    transport_addresses::TransportAddresses,
};

/// Opens a QMux session over `wss://`. `crypto` must offer the HTTP/1.1 ALPN.
pub(crate) async fn connect(
    target: &ConnectTarget,
    crypto: Arc<rustls::ClientConfig>,
    protocols: ApplicationProtocols,
) -> anyhow::Result<QmuxConnection> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(target, crypto, protocols))
        .await
        .context("WebSocket handshake timed out")?
}

async fn handshake(
    target: &ConnectTarget,
    crypto: Arc<rustls::ClientConfig>,
    protocols: ApplicationProtocols,
) -> anyhow::Result<QmuxConnection> {
    let remote_address = target.resolve_remote_address().await?;
    let tcp = TcpStream::connect(remote_address)
        .await
        .with_context(|| format!("TCP connect to {remote_address} failed"))?;
    tcp.set_nodelay(true)?;
    let addresses = TransportAddresses {
        remote: Some(tcp.peer_addr()?),
        local_ip: Some(tcp.local_addr()?.ip()),
    };
    let server_name = ServerName::try_from(target.server_name())?;
    let tls = TlsConnector::from(crypto)
        .connect(server_name, tcp)
        .await
        .context("TLS handshake failed")?;

    let offered = protocols.web_socket_subprotocols();
    let mut request = target.url.as_str().into_client_request()?;
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_str(&offered.join(", "))?,
    );
    let (stream, response) = client_async_with_config(request, tls, Some(web_socket_config()))
        .await
        .context("WebSocket handshake failed")?;
    // draft-lcurley-qmux-websocket-00 §5: no selection, or one this side did
    // not offer, fails the handshake.
    let selected = response
        .headers()
        .get(SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .filter(|selected| offered.iter().any(|offer| offer == selected))
        .map(str::to_string)
        .context("the server selected no offered WebSocket subprotocol")?;
    let alpn = subprotocol::alpn_of(&selected)
        .flatten()
        .map(str::to_string);
    tracing::debug!(%selected, "WebSocket upgraded");

    let (sink, source) = split(stream);
    QmuxConnection::establish(sink, source, Role::Client, alpn, addresses).await
}

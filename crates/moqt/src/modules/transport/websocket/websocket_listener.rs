use std::{
    net::{Ipv6Addr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Context;
use quinn::rustls;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::{HeaderValue, StatusCode, header::SEC_WEBSOCKET_PROTOCOL},
    },
};

use super::{HANDSHAKE_TIMEOUT, HTTP11_ALPN, tungstenite_transport::split, web_socket_config};
use crate::modules::{
    executor::{self, JoinHandle},
    transport::{
        accept_error::AcceptError,
        application_protocol::ApplicationProtocols,
        qmux::{qmux_connection::QmuxConnection, stream_id::Role, subprotocol},
        server_transport_config::load_cert_and_key,
        transport_addresses::TransportAddresses,
    },
};

/// Established sessions waiting for `accept`.
const ACCEPT_BACKLOG: usize = 64;

/// Accepts `wss://` connections on a TCP port: TLS, then the HTTP/1.1 upgrade
/// that selects the QMux subprotocol, then the QMux session. Handshakes run
/// concurrently so a slow one does not delay the others.
pub(crate) struct WebSocketListener {
    accepted: tokio::sync::Mutex<mpsc::Receiver<QmuxConnection>>,
    accept_task: JoinHandle,
}

impl WebSocketListener {
    /// Binds `[::]:port`, which also takes IPv4. `refusing` is read at every
    /// upgrade; while set, the upgrade is answered with 503.
    pub(crate) fn bind(
        port: u16,
        cert_path: &str,
        key_path: &str,
        protocols: ApplicationProtocols,
        refusing: Arc<AtomicBool>,
    ) -> anyhow::Result<Self> {
        let (certs, key) = load_cert_and_key(cert_path, key_path)?;
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .context("failed to build the TLS server config")?;
        server_crypto.alpn_protocols = vec![HTTP11_ALPN.to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(server_crypto));

        let listener = std::net::TcpListener::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
            .with_context(|| format!("failed to bind TCP port {port} for WebSocket"))?;
        listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(listener)?;
        tracing::info!(address = %listener.local_addr()?, "WebSocket server ready");

        let (accepted_sender, accepted_receiver) = mpsc::channel(ACCEPT_BACKLOG);
        let accept_task = executor::spawn(
            "WebSocket Acceptor",
            accept_loop(listener, acceptor, protocols, refusing, accepted_sender),
        );
        Ok(Self {
            accepted: tokio::sync::Mutex::new(accepted_receiver),
            accept_task,
        })
    }

    pub(crate) async fn accept(&self) -> Result<QmuxConnection, AcceptError> {
        self.accepted
            .lock()
            .await
            .recv()
            .await
            .ok_or(AcceptError::EndpointClosing)
    }
}

/// The accept loop never returns, and a dropped handle detaches the task, so
/// it is stopped here; otherwise the TCP port stays bound after the endpoint.
impl Drop for WebSocketListener {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    protocols: ApplicationProtocols,
    refusing: Arc<AtomicBool>,
    accepted_sender: mpsc::Sender<QmuxConnection>,
) {
    loop {
        let (tcp, remote) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "WebSocket TCP accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let refusing = refusing.clone();
        let accepted_sender = accepted_sender.clone();
        executor::spawn("WebSocket Handshake", async move {
            let handshake = handshake(tcp, acceptor, protocols, refusing);
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
                Ok(Ok(connection)) => {
                    let _ = accepted_sender.send(connection).await;
                }
                Ok(Err(error)) => {
                    tracing::debug!(%error, %remote, "WebSocket handshake failed");
                }
                Err(_) => tracing::debug!(%remote, "WebSocket handshake timed out"),
            }
        });
    }
}

async fn handshake(
    tcp: TcpStream,
    acceptor: TlsAcceptor,
    protocols: ApplicationProtocols,
    refusing: Arc<AtomicBool>,
) -> anyhow::Result<QmuxConnection> {
    tcp.set_nodelay(true)?;
    let addresses = TransportAddresses {
        remote: Some(tcp.peer_addr()?),
        local_ip: Some(tcp.local_addr()?.ip()),
    };
    let tls = acceptor.accept(tcp).await.context("TLS handshake failed")?;

    let selected = Arc::new(Mutex::new(None));
    let callback = {
        let selected = selected.clone();
        // The signature is tungstenite's `Callback`.
        #[allow(clippy::result_large_err)]
        move |request: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
            if refusing.load(Ordering::SeqCst) {
                return Err(error_response(StatusCode::SERVICE_UNAVAILABLE, "draining"));
            }
            let offered = request
                .headers()
                .get(SEC_WEBSOCKET_PROTOCOL)
                .and_then(|value| value.to_str().ok())
                .map(subprotocol::offered)
                .unwrap_or_default();
            let Some(chosen) = protocols.select_web_socket(&offered) else {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "no supported QMux subprotocol offered",
                ));
            };
            response.headers_mut().insert(
                SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_str(&chosen).expect("a subprotocol is an HTTP token"),
            );
            *selected.lock().expect("selected subprotocol lock") = Some(chosen);
            Ok(response)
        }
    };
    let stream = accept_hdr_async_with_config(tls, callback, Some(web_socket_config()))
        .await
        .context("WebSocket upgrade failed")?;
    let chosen = selected
        .lock()
        .expect("selected subprotocol lock")
        .take()
        .expect("the upgrade callback selected a subprotocol");
    let alpn = subprotocol::alpn_of(&chosen).flatten().map(str::to_string);
    tracing::debug!(subprotocol = %chosen, remote = ?addresses.remote, "WebSocket upgraded");

    let (sink, source) = split(stream);
    QmuxConnection::establish(sink, source, Role::Server, alpn, addresses).await
}

fn error_response(status: StatusCode, reason: &str) -> ErrorResponse {
    let mut response = ErrorResponse::new(Some(reason.to_string()));
    *response.status_mut() = status;
    response
}

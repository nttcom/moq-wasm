mod logging;
pub use logging::{LoggingGuards, init_logging};
pub mod modules;
mod relay_server;

pub use relay_server::server::RelayServer;
use tokio::sync::oneshot::Receiver;

pub fn run_relay_server<T: moqt::TransportProtocol>(
    port: u16,
    shutdown_signal: Receiver<()>,
    key_path: &str,
    cert_path: &str,
) -> tokio::task::JoinHandle<()> {
    tracing::info!("key_path: {}", key_path);
    tracing::info!("cert_path: {}", cert_path);
    let key_path = key_path.to_string();
    let cert_path = cert_path.to_string();

    tokio::task::Builder::new()
        .name("RelayServer")
        .spawn(async move {
            tracing::info!("Relay server started");
            let server = RelayServer::new(&key_path, &cert_path);
            let handler = server.spawn_transport::<T>(port);

            shutdown_signal.await.ok();
            drop(handler);
            tracing::info!("Relay server shutting down");
        })
        .unwrap()
}

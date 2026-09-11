use std::{net::UdpSocket, path::Path, time::Duration};

use rcgen::{CertifiedKey, generate_simple_self_signed};

use crate::{
    ClientConfig, DUAL, DataReceiver, Endpoint, FilterType, Handshake, ServerConfig, Session,
    SessionEvent, Subscription, TrackReader,
};

pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn free_udp_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

fn write_self_signed_cert(dir: &Path) -> ServerConfig {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, signing_key.serialize_pem()).unwrap();
    ServerConfig {
        port: 0,
        cert_path: cert_path.to_string_lossy().into_owned(),
        key_path: key_path.to_string_lossy().into_owned(),
        keep_alive_interval_sec: 5,
    }
}

/// Starts a DUAL server on a free port whose accept loop resolves the first
/// incoming handshake (CLIENT_SETUP received, SERVER_SETUP not yet sent);
/// returns the port and the accept task.
pub(crate) fn spawn_dual_server_handshake(
    name: &str,
) -> (u16, tokio::task::JoinHandle<Handshake<DUAL>>) {
    let port = free_udp_port();
    let cert_dir = std::env::temp_dir().join(format!("moqt-test-{name}-{port}"));
    let mut server_config = write_self_signed_cert(&cert_dir);
    server_config.port = port;
    let mut server = Endpoint::<DUAL>::create_server(&server_config).unwrap();
    let accept = tokio::spawn(async move { server.accept().await.unwrap().await.unwrap() });
    (port, accept)
}

/// Starts a DUAL server on a free port whose accept loop resolves the first
/// incoming session; returns the port and the accept task.
pub(crate) fn spawn_dual_server(name: &str) -> (u16, tokio::task::JoinHandle<Session<DUAL>>) {
    let (port, handshake) = spawn_dual_server_handshake(name);
    let accept = tokio::spawn(async move { handshake.await.unwrap().accept().await.unwrap() });
    (port, accept)
}

pub(crate) fn dual_client_with_config(config: ClientConfig) -> Endpoint<DUAL> {
    Endpoint::<DUAL>::create_client(&config).unwrap()
}

pub(crate) fn dual_client() -> Endpoint<DUAL> {
    dual_client_with_config(ClientConfig {
        port: 0,
        verify_certificate: false,
        authorization_token: None,
    })
}

/// Connects a DUAL client to the server started by `spawn_dual_server` and
/// returns both established sessions as (client, server).
pub(crate) async fn connect_sessions(
    url: &str,
    accept: tokio::task::JoinHandle<Session<DUAL>>,
) -> anyhow::Result<(Session<DUAL>, Session<DUAL>)> {
    let client = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        dual_client().connect(url).await?.await
    })
    .await??;
    let server = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept).await??;
    Ok((client, server))
}

/// Answers the client's PUBLISH with PUBLISH_OK and registers the object
/// receiver for its track alias.
pub(crate) async fn accept_publish(server: &Session<DUAL>) -> Subscription {
    let SessionEvent::Publish(handler) = server.receive_event().await.unwrap() else {
        panic!("expected PUBLISH from the client");
    };
    let subscription = handler.ok(128, FilterType::LargestObject, 0).await.unwrap();
    handler.accept_data_receiver().await;
    subscription
}

/// The data receiver resolves only once the first object has arrived, so
/// this must run after the publisher has sent something.
pub(crate) async fn subscribed_track_reader(
    server: &Session<DUAL>,
    subscription: &Subscription,
) -> TrackReader<DUAL> {
    let DataReceiver::Stream(factory) = server
        .subscriber()
        .accept_data_receiver(subscription)
        .await
        .unwrap()
    else {
        panic!("expected a subgroup stream receiver");
    };
    TrackReader::new(factory)
}

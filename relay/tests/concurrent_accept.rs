//! Regression test for the serial accept-loop bug.
//!
//! The relay accept loop must not let a single client that completes the QUIC
//! handshake but is slow to send its `ClientSetup` block the establishment of
//! other, unrelated connections.

use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::time::Duration;

use moqt::{ClientConfig, Endpoint, QUIC};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use relay::RelayServer;

/// Grabs an ephemeral UDP port, then releases it so the relay can bind it.
fn free_udp_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.local_addr().unwrap().port()
}

/// Writes a fresh self-signed cert/key into `dir` so the test does not depend on
/// the (gitignored) relay/keys artifacts. Returns (key_path, cert_path).
fn generate_certs(dir: &Path) -> (String, String) {
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, signing_key.serialize_pem()).unwrap();
    (
        key_path.to_string_lossy().into_owned(),
        cert_path.to_string_lossy().into_owned(),
    )
}

fn client_endpoint() -> Endpoint<QUIC> {
    Endpoint::<QUIC>::create_client(&ClientConfig {
        port: 0,
        verify_certificate: false,
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_client_setup_does_not_block_new_connections() {
    let port = free_udp_port();
    let remote: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let host = "127.0.0.1";

    let cert_dir = std::env::temp_dir().join(format!("relay-concurrent-accept-{port}"));
    let (key_path, cert_path) = generate_certs(&cert_dir);

    let server = RelayServer::new(&key_path, &cert_path);
    // Keep the handler alive for the duration of the test (Drop aborts it).
    let _handler = server.spawn_client_transport::<QUIC>(port);

    // Client A: complete the QUIC handshake, then hold the connection open
    // without ever opening the control stream / sending ClientSetup. The
    // returned `Connecting` future is intentionally NOT awaited.
    let endpoint_a = client_endpoint();
    let _connecting_a = endpoint_a
        .connect(remote, host)
        .await
        .expect("client A QUIC handshake should succeed");

    // Client B: a well-behaved client that should be able to fully establish a
    // session even while A sits idle after its handshake.
    let endpoint_b = client_endpoint();
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        endpoint_b.connect(remote, host).await?.await
    })
    .await;

    match result {
        Err(_) => panic!(
            "client B timed out establishing a session; a slow client A blocked the accept loop"
        ),
        Ok(Err(e)) => panic!("client B failed to establish a session: {e:#}"),
        Ok(Ok(_session)) => { /* success */ }
    }
}

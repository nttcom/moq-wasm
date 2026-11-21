use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use moqt::{
    CertificateVerification, Endpoint, NATIVE, SessionEvent, TrackReader,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
    subscribe::SubscribeOption,
};
use tokio::sync::{mpsc, oneshot};

use crate::{ConnectOptions, Session, connect};

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A relay that accepts one session, answers its PUBLISH_NAMESPACE and
/// hands its PUBLISH_NAMESPACE and PUBLISH_NAMESPACE_DONE to `next_request`.
pub(crate) struct TestRelay {
    url: String,
    session_receiver: oneshot::Receiver<Arc<moqt::Session>>,
    request_receiver: mpsc::UnboundedReceiver<String>,
}

impl TestRelay {
    pub(crate) fn spawn(name: &str) -> Self {
        let endpoint = Endpoint::<NATIVE>::create_server(&self_signed_server_config(name)).unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let (session_sender, session_receiver) = oneshot::channel();
        let (request_sender, request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let session = Arc::new(handshake.accept(&tracing::Span::current()).await.unwrap());
            let _ = session_sender.send(session.clone());
            while let Ok(event) = session.receive_event().await {
                match event {
                    SessionEvent::PublishNamespace(handler) => {
                        let _ = request_sender
                            .send(format!("PUBLISH_NAMESPACE {}", handler.track_namespace));
                        tokio::spawn(async move { handler.ok().await.unwrap() });
                    }
                    SessionEvent::PublishNamespaceDone(handler) => {
                        let _ = request_sender.send(format!(
                            "PUBLISH_NAMESPACE_DONE {}",
                            handler.track_namespace()
                        ));
                    }
                    SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
                    _ => {}
                }
            }
        });
        Self {
            url,
            session_receiver,
            request_receiver,
        }
    }

    pub(crate) async fn connect(&self) -> Session {
        let options = ConnectOptions {
            certificate_verification: CertificateVerification::Disabled,
            ..ConnectOptions::default()
        };
        connect(&self.url, options).await.unwrap()
    }

    pub(crate) async fn next_request(&mut self) -> String {
        tokio::time::timeout(TEST_TIMEOUT, self.request_receiver.recv())
            .await
            .expect("no request arrived")
            .expect("the relay stopped")
    }

    /// Subscribes to the track, runs `write` once the SUBSCRIBE is answered
    /// and returns the first object's payload.
    pub(crate) async fn next_object_after(
        &mut self,
        namespace: &str,
        track_name: &str,
        write: impl Future<Output = ()>,
    ) -> Bytes {
        let session = (&mut self.session_receiver).await.unwrap();
        let mut subscriber = session.subscriber();
        let subscription = subscriber
            .subscribe(
                namespace.to_string(),
                track_name.to_string(),
                SubscribeOption::default(),
            )
            .await
            .unwrap();
        write.await;
        tokio::time::timeout(TEST_TIMEOUT, async {
            let receiver = subscriber
                .accept_data_receiver(&subscription)
                .await
                .unwrap();
            TrackReader::new(receiver)
                .next_object()
                .await
                .unwrap()
                .unwrap()
                .payload
        })
        .await
        .expect("no object arrived")
    }
}

/// A server config with a self-signed certificate under a directory named
/// after `name`, so tests running in parallel do not share one.
fn self_signed_server_config(name: &str) -> ServerConfig {
    let dir = std::env::temp_dir().join(format!("moq-wasm-test-{name}-{}", std::process::id()));
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&dir, &["localhost", "127.0.0.1"]).unwrap();
    ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
        versions: moqt::DEFAULT_VERSIONS.to_vec(),
    }
}

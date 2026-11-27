use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use moqt::{
    CertificateVerification, Endpoint, NATIVE, SessionEvent, TrackReader, Version,
    object::ExtensionHeaders,
    publish::{GroupBoundary, Origin, OutgoingObject, Retention},
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
    subscribe::{FilterType, SubscribeOption},
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{ConnectOptions, Session, connect};

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A draft-14 relay for one session. It answers the session's
/// PUBLISH_NAMESPACE and SUBSCRIBE_NAMESPACE, echoes the session's
/// namespaces back once it subscribed to namespaces, as a relay does
/// (draft-14 §6.1), accepts every PUBLISH, serves SUBSCRIBEs from `origin`,
/// and hands the namespace requests and PUBLISHes it receives to
/// `next_request`.
pub(crate) struct TestRelay {
    url: String,
    session_receiver: Option<oneshot::Receiver<Arc<moqt::Session>>>,
    session: Option<Arc<moqt::Session>>,
    request_receiver: mpsc::UnboundedReceiver<String>,
    origin: Origin,
}

impl TestRelay {
    pub(crate) fn spawn(name: &str) -> Self {
        let endpoint = Endpoint::<NATIVE>::create_server(&self_signed_server_config(name)).unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let (session_sender, session_receiver) = oneshot::channel();
        let (request_sender, request_receiver) = mpsc::unbounded_channel();
        let origin = Origin::default();
        let serving = origin.clone();
        tokio::spawn(async move {
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let session = Arc::new(handshake.accept(&tracing::Span::current()).await.unwrap());
            let _ = session_sender.send(session.clone());
            let (watching_sender, watching_receiver) = watch::channel(false);
            while let Ok(event) = session.receive_event().await {
                match event {
                    SessionEvent::PublishNamespace(handler) => {
                        let namespace = handler.track_namespace.clone();
                        let _ = request_sender.send(format!("PUBLISH_NAMESPACE {namespace}"));
                        let session = session.clone();
                        let mut watching = watching_receiver.clone();
                        tokio::spawn(async move {
                            handler.ok().await.unwrap();
                            if watching.wait_for(|watching| *watching).await.is_ok() {
                                let _ = session.publisher().publish_namespace(namespace).await;
                            }
                        });
                    }
                    SessionEvent::PublishNamespaceDone(handler) => {
                        let _ = request_sender.send(format!(
                            "PUBLISH_NAMESPACE_DONE {}",
                            handler.track_namespace
                        ));
                    }
                    SessionEvent::SubscribeNameSpace(handler) => {
                        let _ = request_sender.send(format!(
                            "SUBSCRIBE_NAMESPACE {}",
                            handler.track_namespace_prefix
                        ));
                        handler.ok().await.unwrap();
                        watching_sender.send_replace(true);
                    }
                    SessionEvent::Publish(handler) => {
                        let _ = request_sender.send(format!(
                            "PUBLISH {}/{}",
                            handler.track_namespace, handler.track_name
                        ));
                        tokio::spawn(async move {
                            handler.accept_data_receiver().await;
                            let _ = handler
                                .ok(0, FilterType::LargestObject, handler.forward)
                                .await;
                        });
                    }
                    SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
                    event => {
                        serving.handle(event).await;
                    }
                }
            }
        });
        Self {
            url,
            session_receiver: Some(session_receiver),
            session: None,
            request_receiver,
            origin,
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

    /// Publishes `namespace` to the session, as another publisher behind
    /// the relay would.
    pub(crate) async fn publish_namespace(&mut self, namespace: &str) {
        let session = self.session().await;
        session
            .publisher()
            .publish_namespace(namespace.to_string())
            .await
            .unwrap();
    }

    pub(crate) async fn publish_namespace_done(&mut self, namespace: &str) {
        let session = self.session().await;
        session
            .publisher()
            .publish_namespace_done(namespace.to_string())
            .await
            .unwrap();
    }

    /// Writes an object of the track the relay serves, in a new group when
    /// `new_group`.
    pub(crate) async fn write(
        &self,
        namespace: &str,
        track_name: &str,
        payload: &[u8],
        new_group: bool,
    ) {
        self.origin
            .ensure_track(namespace, track_name, 0, Retention::For(TEST_TIMEOUT))
            .await;
        let object = OutgoingObject {
            group: if new_group {
                GroupBoundary::Next
            } else {
                GroupBoundary::Within
            },
            extension_headers: ExtensionHeaders::default(),
            payload: Bytes::copy_from_slice(payload),
        };
        self.origin
            .write(namespace, track_name, object)
            .await
            .unwrap();
    }

    /// Subscribes to the track, runs `write` once the SUBSCRIBE is answered
    /// and returns the first object's payload.
    pub(crate) async fn next_object_after(
        &mut self,
        namespace: &str,
        track_name: &str,
        write: impl Future<Output = ()>,
    ) -> Bytes {
        let session = self.session().await;
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

    async fn session(&mut self) -> Arc<moqt::Session> {
        if let Some(receiver) = self.session_receiver.take() {
            self.session = Some(receiver.await.unwrap());
        }
        self.session.clone().unwrap()
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
        versions: vec![Version::Draft14],
    }
}

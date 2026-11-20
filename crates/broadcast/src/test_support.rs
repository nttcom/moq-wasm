use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use moqt::{
    CertificateVerification, ClientConfig, Endpoint, NATIVE, Session, SessionEvent, Subscription,
    TrackObject, TrackReader,
    publish::Origin,
    server::{CertificateFiles, ServerConfig, write_self_signed_cert},
    subscribe::FilterType,
};
use msf::CATALOG_TRACK_NAME;
use tokio::sync::{mpsc, oneshot};

use crate::{
    broadcast::VIDEO_TRACK_NAME,
    relay_connection::{MoqtTarget, RelayConnection},
};

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// A server config with a self-signed certificate written under a temporary
/// directory named after `name`, so tests running in parallel do not share one.
pub(crate) fn self_signed_server_config(name: &str) -> ServerConfig {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("broadcast-test-{name}-{}", std::process::id()));
    let CertificateFiles {
        cert_path,
        key_path,
    } = write_self_signed_cert(&dir, &["localhost", "127.0.0.1"]).unwrap();
    ServerConfig {
        port: 0,
        cert_path,
        key_path,
        keep_alive_interval_sec: 5,
        // The tests write and read draft-14 messages.
        versions: vec![moqt::Version::Draft14],
    }
}

/// A client session to `url` that trusts any certificate.
pub(crate) async fn connect(url: &str) -> Arc<Session> {
    let endpoint = Endpoint::<NATIVE>::create_client(&ClientConfig {
        certificate_verification: CertificateVerification::Disabled,
        ..ClientConfig::default()
    })
    .unwrap();
    Arc::new(endpoint.connect(url).await.unwrap().await.unwrap())
}

/// A publisher that serves the tracks registered with `origin` to the one
/// session that connects to `url`, as a relay would.
pub(crate) struct FakePublisher {
    pub(crate) url: String,
    pub(crate) origin: Origin,
    /// The track name of each SUBSCRIBE, sent once `origin` serves it.
    served: mpsc::UnboundedReceiver<String>,
}

impl FakePublisher {
    pub(crate) fn spawn() -> Self {
        let endpoint =
            Endpoint::<NATIVE>::create_server(&self_signed_server_config("publisher")).unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let origin = Origin::default();
        let (served_sender, served) = mpsc::unbounded_channel();
        let serving = origin.clone();
        tokio::spawn(async move {
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let session = handshake.accept(&tracing::Span::current()).await.unwrap();
            while let Ok(event) = session.receive_event().await {
                let subscribed = match &event {
                    SessionEvent::Subscribe(handler) => Some(handler.track_name.clone()),
                    _ => None,
                };
                serving.handle(event).await;
                if let Some(track_name) = subscribed {
                    let _ = served_sender.send(track_name);
                }
            }
        });
        Self {
            url,
            origin,
            served,
        }
    }

    /// The track name of the next SUBSCRIBE `origin` serves.
    ///
    /// # Panics
    /// When none arrives within 5 seconds, as when the subscriber gave up.
    pub(crate) async fn next_served(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(5), self.served.recv())
            .await
            .expect("no track was subscribed")
            .expect("the publisher stopped serving")
    }
}

/// A relay that accepts one publisher session and answers its
/// PUBLISH_NAMESPACE and PUBLISH.
pub(crate) struct FakeRelay {
    pub(crate) url: String,
    pub(crate) session: oneshot::Receiver<Arc<Session>>,
    pub(crate) requests: mpsc::UnboundedReceiver<String>,
    pub(crate) published: mpsc::UnboundedReceiver<Subscription>,
    pub(crate) video_objects: mpsc::UnboundedReceiver<TrackObject>,
    pub(crate) catalog_objects: mpsc::UnboundedReceiver<TrackObject>,
}

impl FakeRelay {
    pub(crate) fn spawn(name: &str) -> Self {
        Self::spawn_gated(name, None, None)
    }

    /// A connection that opens its own session to this relay on first use.
    pub(crate) fn connection(&self) -> RelayConnection {
        RelayConnection::new(Some(MoqtTarget {
            url: self.url.clone(),
            auth_token: None,
            verify_certificate: false,
        }))
    }

    /// PUBLISH_NAMESPACE is answered only once `namespace_gate` fires;
    /// the catalog track is read only once `catalog_gate` fires.
    pub(crate) fn spawn_gated(
        name: &str,
        namespace_gate: Option<oneshot::Receiver<()>>,
        catalog_gate: Option<oneshot::Receiver<()>>,
    ) -> Self {
        let endpoint = Endpoint::<NATIVE>::create_server(&self_signed_server_config(name)).unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let (session_sender, session) = oneshot::channel();
        let (request_sender, requests) = mpsc::unbounded_channel();
        let (published_sender, published) = mpsc::unbounded_channel();
        let (video_object_sender, video_objects) = mpsc::unbounded_channel();
        let (catalog_object_sender, catalog_objects) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (mut namespace_gate, mut catalog_gate) = (namespace_gate, catalog_gate);
            let mut published_tracks = HashMap::new();
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let session = Arc::new(handshake.accept(&tracing::Span::current()).await.unwrap());
            let _ = session_sender.send(session.clone());
            while let Ok(event) = session.receive_event().await {
                match event {
                    SessionEvent::PublishNamespace(handler) => {
                        let _ = request_sender
                            .send(format!("PUBLISH_NAMESPACE {}", handler.track_namespace));
                        // Answered aside, so requests sent meanwhile are still recorded
                        let gate = namespace_gate.take();
                        tokio::spawn(async move {
                            if let Some(gate) = gate {
                                let _ = gate.await;
                            }
                            handler.ok().await.unwrap();
                        });
                    }
                    SessionEvent::Publish(handler) => {
                        let _ = request_sender.send(format!(
                            "PUBLISH {}/{}",
                            handler.track_namespace, handler.track_name
                        ));
                        let object_sender = match handler.track_name.as_str() {
                            VIDEO_TRACK_NAME => Some(video_object_sender.clone()),
                            CATALOG_TRACK_NAME => Some(catalog_object_sender.clone()),
                            _ => None,
                        };
                        if object_sender.is_some() {
                            handler.accept_data_receiver().await;
                        }
                        let subscription = handler
                            .ok(0, FilterType::LargestObject, handler.forward)
                            .await
                            .unwrap();
                        published_tracks.insert(
                            subscription.request_id(),
                            format!("{}/{}", handler.track_namespace, handler.track_name),
                        );
                        if let Some(sender) = object_sender {
                            let session = session.clone();
                            let subscription = subscription.clone();
                            let gate = (handler.track_name == CATALOG_TRACK_NAME)
                                .then(|| catalog_gate.take())
                                .flatten();
                            tokio::spawn(async move {
                                if let Some(gate) = gate {
                                    let _ = gate.await;
                                }
                                let Ok(receiver) = session
                                    .subscriber()
                                    .accept_data_receiver(&subscription)
                                    .await
                                else {
                                    return;
                                };
                                let mut reader = TrackReader::new(receiver);
                                while let Ok(Some(object)) = reader.next_object().await {
                                    let _ = sender.send(object);
                                }
                            });
                        }
                        let _ = published_sender.send(subscription);
                    }
                    SessionEvent::PublishDone(handler) => {
                        if let Some(track) = published_tracks.get(&handler.request_id()) {
                            let _ = request_sender.send(format!("PUBLISH_DONE {track}"));
                        }
                    }
                    SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
                    _ => {}
                }
            }
        });
        Self {
            url,
            session,
            requests,
            published,
            video_objects,
            catalog_objects,
        }
    }

    /// Mimics a draining relay whose subscribers have left.
    pub(crate) async fn unsubscribe_published(&mut self, session: &Session) {
        while let Ok(subscription) = self.published.try_recv() {
            session
                .subscriber()
                .unsubscribe(&subscription)
                .await
                .unwrap();
        }
    }

    pub(crate) async fn wait_for_video_group(&mut self, group_id: u64) {
        let mut seen = Vec::new();
        let arrived = tokio::time::timeout(TEST_TIMEOUT, async {
            while let Some(object) = self.video_objects.recv().await {
                seen.push(object.group_id);
                if object.group_id == group_id {
                    return;
                }
            }
        })
        .await;
        assert!(
            arrived.is_ok(),
            "video group {group_id} must arrive at {}, saw {seen:?}",
            self.url
        );
    }

    pub(crate) async fn session(&mut self) -> Arc<Session> {
        tokio::time::timeout(TEST_TIMEOUT, &mut self.session)
            .await
            .expect("the publisher must connect")
            .unwrap()
    }

    pub(crate) async fn wait_for_request(&mut self, expected: &str) {
        let mut seen = Vec::new();
        let arrived = tokio::time::timeout(TEST_TIMEOUT, async {
            while let Some(request) = self.requests.recv().await {
                let found = request == expected;
                seen.push(request);
                if found {
                    return;
                }
            }
        })
        .await;
        assert!(arrived.is_ok(), "{expected} must arrive, saw {seen:?}");
    }

    pub(crate) async fn received_catalogs(&mut self) -> Vec<TrackObject> {
        let mut catalogs = Vec::new();
        while let Ok(Some(object)) =
            tokio::time::timeout(Duration::from_millis(300), self.catalog_objects.recv()).await
        {
            catalogs.push(object);
        }
        catalogs
    }

    pub(crate) fn received_requests(&mut self) -> Vec<String> {
        let mut requests = Vec::new();
        while let Ok(request) = self.requests.try_recv() {
            requests.push(request);
        }
        requests
    }
}

/// The names of the tracks a catalog object lists, in catalog order.
pub(crate) fn catalog_track_names(object: &TrackObject) -> Vec<String> {
    let catalog: msf::Catalog = serde_json::from_slice(&object.payload).unwrap();
    catalog
        .tracks
        .into_iter()
        .flatten()
        .map(|track| track.name)
        .collect()
}

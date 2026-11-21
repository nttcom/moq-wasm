use std::{collections::HashMap, sync::Arc, time::Duration};

use moqt::{
    AuthorizationToken, Endpoint, NATIVE, Session, SessionEvent, Subscription, TrackObject,
    TrackReader, Version,
    object::{DataReceiver, Subgroup, SubgroupStart, TrackData},
    server::ServerConfig,
    subscribe::{FilterType, SubscribeOption},
};
use msf::{CATALOG_TRACK_NAME, Catalog};
use tokio::sync::{mpsc, oneshot};

use super::{TEST_TIMEOUT, self_signed_server_config};
use crate::{
    broadcast::VIDEO_TRACK_NAME,
    relay_connection::{MoqtTarget, RelayConnection},
};

/// A relay that accepts one publisher session and answers its
/// PUBLISH_NAMESPACE and PUBLISH.
pub(crate) struct FakeRelay {
    pub(crate) url: String,
    pub(crate) session: oneshot::Receiver<Arc<Session>>,
    connected_session: Option<Arc<Session>>,
    /// The AUTHORIZATION TOKEN parameters of the publisher's CLIENT_SETUP.
    pub(crate) authorization_tokens: oneshot::Receiver<Vec<AuthorizationToken>>,
    pub(crate) requests: mpsc::UnboundedReceiver<String>,
    pub(crate) published: mpsc::UnboundedReceiver<Subscription>,
    video_objects: mpsc::UnboundedReceiver<TrackObject>,
    pub(crate) catalog_objects: mpsc::UnboundedReceiver<TrackObject>,
    /// The data of every other published track, by track name.
    track_data: mpsc::UnboundedReceiver<(String, DataReceiver)>,
}

impl FakeRelay {
    pub(crate) fn spawn(name: &str) -> Self {
        Self::spawn_gated(name, None, None)
    }

    pub(crate) fn spawn_draft18(name: &str) -> Self {
        Self::spawn_with(name, Version::Draft18, None, None)
    }

    /// PUBLISH_NAMESPACE is answered only once `namespace_gate` fires;
    /// the catalog track is read only once `catalog_gate` fires.
    pub(crate) fn spawn_gated(
        name: &str,
        namespace_gate: Option<oneshot::Receiver<()>>,
        catalog_gate: Option<oneshot::Receiver<()>>,
    ) -> Self {
        Self::spawn_with(name, Version::Draft14, namespace_gate, catalog_gate)
    }

    fn spawn_with(
        name: &str,
        version: Version,
        namespace_gate: Option<oneshot::Receiver<()>>,
        catalog_gate: Option<oneshot::Receiver<()>>,
    ) -> Self {
        let config = ServerConfig {
            versions: vec![version],
            ..self_signed_server_config(name)
        };
        let endpoint = Endpoint::<NATIVE>::create_server(&config).unwrap();
        let url = format!("moqt://127.0.0.1:{}", endpoint.local_port().unwrap());
        let (session_sender, session) = oneshot::channel();
        let (authorization_tokens_sender, authorization_tokens) = oneshot::channel();
        let (request_sender, requests) = mpsc::unbounded_channel();
        let (published_sender, published) = mpsc::unbounded_channel();
        let (video_object_sender, video_objects) = mpsc::unbounded_channel();
        let (catalog_object_sender, catalog_objects) = mpsc::unbounded_channel();
        let (track_data_sender, track_data) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (mut namespace_gate, mut catalog_gate) = (namespace_gate, catalog_gate);
            let mut published_tracks = HashMap::new();
            let handshake = endpoint.accept().await.unwrap().await.unwrap();
            let _ = authorization_tokens_sender.send(handshake.authorization_tokens().to_vec());
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
                        handler.accept_data_receiver().await;
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
                        } else if let Ok(receiver) = session
                            .subscriber()
                            .accept_data_receiver(&subscription)
                            .await
                        {
                            let _ = track_data_sender.send((handler.track_name.clone(), receiver));
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
            connected_session: None,
            authorization_tokens,
            requests,
            published,
            video_objects,
            catalog_objects,
            track_data,
        }
    }

    /// A connection that opens its session to this relay on first use.
    pub(crate) fn connection(&self) -> RelayConnection {
        RelayConnection::new(Some(MoqtTarget {
            url: self.url.clone(),
            auth_token: None,
            verify_certificate: false,
        }))
    }

    /// A SUBSCRIBE as a viewer behind the relay would send.
    pub(crate) async fn subscribe(&mut self, namespace: &str, track_name: &str) -> Subscription {
        let session = self.session().await;
        tokio::time::timeout(
            TEST_TIMEOUT,
            session.subscriber().subscribe(
                namespace.to_string(),
                track_name.to_string(),
                SubscribeOption::default(),
            ),
        )
        .await
        .expect("the SUBSCRIBE must be answered")
        .unwrap()
    }

    pub(crate) async fn first_object(&mut self, subscription: &Subscription) -> TrackObject {
        let session = self.session().await;
        tokio::time::timeout(TEST_TIMEOUT, async {
            let receiver = session
                .subscriber()
                .accept_data_receiver(subscription)
                .await
                .unwrap();
            TrackReader::new(receiver).next_object().await.unwrap()
        })
        .await
        .expect("an object must arrive")
        .expect("the track must not end before its first object")
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
        if let Some(session) = &self.connected_session {
            return session.clone();
        }
        let session = tokio::time::timeout(TEST_TIMEOUT, &mut self.session)
            .await
            .expect("the publisher must connect")
            .unwrap();
        self.connected_session = Some(session.clone());
        session
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

    pub(crate) async fn newest_catalog(&mut self) -> Catalog {
        let catalogs = self.received_catalogs().await;
        let newest = catalogs
            .iter()
            .max_by_key(|object| object.group_id)
            .expect("a catalog must arrive");
        serde_json::from_slice(&newest.payload).unwrap()
    }

    async fn track_receiver(&mut self, track_name: &str) -> DataReceiver {
        loop {
            let (name, receiver) = self.track_data.recv().await.unwrap();
            if name == track_name {
                return receiver;
            }
        }
    }

    pub(crate) async fn first_track_object(&mut self, track_name: &str) -> TrackObject {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let receiver = self.track_receiver(track_name).await;
            TrackReader::new(receiver)
                .next_object()
                .await
                .unwrap()
                .expect("the track must not end before its first object")
        })
        .await
        .expect("an object of the track must arrive")
    }

    /// The header of the first subgroup of the published track `track_name`.
    pub(crate) async fn first_subgroup_header(&mut self, track_name: &str) -> SubgroupStart {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let mut receiver = self.track_receiver(track_name).await;
            let TrackData::Subgroup(mut stream) = receiver.next().await.unwrap() else {
                panic!("{track_name} must be sent on subgroup streams");
            };
            let Some(Subgroup::Header(header)) = stream.receive().await.unwrap() else {
                panic!("a subgroup stream must begin with its header");
            };
            header
        })
        .await
        .expect("a subgroup of the track must arrive")
    }

    pub(crate) fn received_requests(&mut self) -> Vec<String> {
        let mut requests = Vec::new();
        while let Ok(request) = self.requests.try_recv() {
            requests.push(request);
        }
        requests
    }
}

use std::{cell::RefCell, collections::HashMap};

use broadcast::RelayConnection;
use moqt::{
    RequestErrorCode, Session, SessionEvent,
    namespace::{NamespaceEvent, NamespaceWatchers},
    publish::Origin,
    wire::publish_done_code_to_wire,
};
use tokio::sync::{Mutex, mpsc, oneshot, watch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishDone {
    pub(crate) code: u64,
    pub(crate) reason: String,
}

/// Keyed by `(namespace, track)`.
type SubscriberWatchers = HashMap<(String, String), mpsc::UnboundedSender<()>>;

/// Each event goes to the broadcasts first, then to `origin`, which they
/// share. `origin` answers the peer's SUBSCRIBE, FETCH and TRACK_STATUS for
/// the other tracks this session publishes. A SUBSCRIBE for any other track,
/// and every PUBLISH, is rejected.
#[derive(Default)]
pub(crate) struct SessionRoutes {
    pub(crate) origin: Origin,
    broadcasts: RefCell<Vec<RelayConnection>>,
    watchers: NamespaceWatchers,
    publish_done: RefCell<HashMap<u64, oneshot::Sender<PublishDone>>>,
    subscriber_watchers: RefCell<SubscriberWatchers>,
}

/// The namespace events of one watch, until the session ends or `close`.
pub(crate) struct NamespaceEvents {
    events: Mutex<mpsc::UnboundedReceiver<NamespaceEvent>>,
    /// Ends a `next` that waits, since it holds `events` meanwhile.
    closed: watch::Sender<bool>,
}

impl NamespaceEvents {
    pub(crate) async fn next(&self) -> Option<NamespaceEvent> {
        let mut closed = self.closed.subscribe();
        let mut events = self.events.lock().await;
        tokio::select! {
            biased;
            _ = closed.wait_for(|closed| *closed) => None,
            event = events.recv() => event,
        }
    }

    pub(crate) async fn close(&self) {
        self.closed.send_replace(true);
        self.events.lock().await.close();
    }
}

impl SessionRoutes {
    pub(crate) fn watch(&self, prefix: Vec<String>) -> NamespaceEvents {
        NamespaceEvents {
            events: Mutex::new(self.watchers.watch(prefix)),
            closed: watch::channel(false).0,
        }
    }

    /// Fails once the session ends or `forget` drops it.
    pub(crate) fn publish_done(&self, request_id: u64) -> oneshot::Receiver<PublishDone> {
        let (sender, receiver) = oneshot::channel();
        self.publish_done.borrow_mut().insert(request_id, sender);
        receiver
    }

    /// Receives once per SUBSCRIBE `origin` answers for the track; ends with
    /// the session. Watching a track again replaces the earlier watch.
    pub(crate) fn watch_subscribers(
        &self,
        namespace: &str,
        track: &str,
    ) -> mpsc::UnboundedReceiver<()> {
        let (sender, receiver) = mpsc::unbounded_channel();
        self.subscriber_watchers
            .borrow_mut()
            .insert((namespace.to_string(), track.to_string()), sender);
        receiver
    }

    pub(crate) fn add_broadcast(&self, connection: RelayConnection) {
        self.broadcasts.borrow_mut().push(connection);
    }

    pub(crate) fn forget(&self, request_id: u64) {
        self.publish_done.borrow_mut().remove(&request_id);
    }

    /// Returns once the session has ended.
    pub(crate) async fn route(&self, session: &Session) {
        'events: while let Ok(event) = session.receive_event().await {
            let mut event = event;
            let broadcasts = self.broadcasts.borrow().clone();
            for broadcast in broadcasts {
                let Some(unhandled) = broadcast.handle(event).await else {
                    continue 'events;
                };
                event = unhandled;
            }
            let subscribed = match &event {
                SessionEvent::Subscribe(handler) => {
                    Some((handler.track_namespace.clone(), handler.track_name.clone()))
                }
                _ => None,
            };
            let Some(event) = self.origin.handle(event).await else {
                if let Some(track) = subscribed {
                    self.notify_subscribed(&track);
                }
                continue;
            };
            let Some(event) = self.watchers.handle(event).await else {
                continue;
            };
            match event {
                SessionEvent::PublishDone(handler) => {
                    let sender = self.publish_done.borrow_mut().remove(&handler.request_id());
                    if let Some(sender) = sender {
                        let _ = sender.send(PublishDone {
                            code: publish_done_code_to_wire(handler.status_code()),
                            reason: handler.error_reason().to_string(),
                        });
                    }
                }
                SessionEvent::SubscribeNameSpace(handler) => {
                    if let Err(error) = handler.ok().await {
                        tracing::debug!(%error, "failed to acknowledge SUBSCRIBE_NAMESPACE");
                    }
                }
                SessionEvent::Subscribe(handler) => {
                    let reason = format!("{} is not published here", handler.track_name);
                    let _ = handler
                        .error(RequestErrorCode::TrackDoesNotExist, reason)
                        .await;
                }
                SessionEvent::Publish(handler) => {
                    let _ = handler
                        .error(
                            RequestErrorCode::Uninterested,
                            "this session takes no PUBLISH".to_string(),
                        )
                        .await;
                }
                SessionEvent::Disconnected() | SessionEvent::ProtocolViolation() => break,
                event => tracing::debug!(?event, "session event ignored"),
            }
        }
        self.watchers.close();
        self.publish_done.borrow_mut().clear();
        self.subscriber_watchers.borrow_mut().clear();
    }

    fn notify_subscribed(&self, track: &(String, String)) {
        if let Some(sender) = self.subscriber_watchers.borrow().get(track) {
            let _ = sender.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use moqt::{
        ClientConfig, Endpoint, NATIVE, RequestFailure,
        object::ContentExists,
        publish::{GroupBoundary, OutgoingObject, PublishDoneCode, Retention},
        server::{CertificateFiles, ServerConfig, write_self_signed_cert},
        subscribe::SubscribeOption,
    };

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    struct Connected {
        client: Session,
        server: Session,
        _endpoints: (Endpoint<NATIVE>, Endpoint<NATIVE>),
    }

    async fn connected_sessions(name: &str) -> Connected {
        let dir = std::env::temp_dir().join(format!("wasm-routes-{name}-{}", std::process::id()));
        let CertificateFiles {
            cert_path,
            key_path,
        } = write_self_signed_cert(&dir, &["localhost", "127.0.0.1"]).unwrap();
        let server = Endpoint::<NATIVE>::create_server(&ServerConfig {
            port: 0,
            cert_path,
            key_path,
            keep_alive_interval_sec: 5,
        })
        .unwrap();
        let url = format!("moqt://127.0.0.1:{}", server.local_port().unwrap());
        let accept = tokio::spawn(async move {
            let session = server.accept().await.unwrap().await.unwrap().accept().await;
            (server, session.unwrap())
        });
        let client = Endpoint::<NATIVE>::create_client(&ClientConfig {
            verify_certificate: false,
            follow_go_away: false,
            ..ClientConfig::default()
        })
        .unwrap();
        let client_session = client.connect(&url).await.unwrap().await.unwrap();
        let (server, server_session) = accept.await.unwrap();
        Connected {
            client: client_session,
            server: server_session,
            _endpoints: (client, server),
        }
    }

    #[tokio::test]
    async fn closing_a_namespace_watch_ends_the_read_that_waits() {
        // Arrange
        let routes = SessionRoutes::default();
        let events = routes.watch(vec!["room".to_string()]);

        // Act
        let (event, ()) = tokio::time::timeout(TIMEOUT, async {
            tokio::join!(events.next(), events.close())
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(event, None);
        assert_eq!(events.next().await, None);
    }

    #[tokio::test]
    async fn publish_done_reaches_the_waiter_of_its_subscription() {
        // Arrange
        let Connected {
            client,
            server,
            _endpoints,
        } = connected_sessions("publish-done").await;
        let routes = SessionRoutes::default();
        let mut subscriber = client.subscriber();
        let (subscription, served) = tokio::join!(
            subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()),
            async {
                let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
                    panic!("expected SUBSCRIBE");
                };
                let alias = handler.ok(0, ContentExists::False).await.unwrap();
                handler.into_subscription(alias)
            }
        );
        let done = routes.publish_done(subscription.unwrap().request_id());

        // Act
        let received = tokio::time::timeout(TIMEOUT, async {
            tokio::select! {
                () = routes.route(&client) => panic!("the session ended"),
                done = done => done,
            }
        });
        let publisher = server.publisher();
        let (received, sent) = tokio::join!(
            received,
            publisher.publish_done(
                &served,
                PublishDoneCode::TrackEnded,
                0,
                "finished".to_string()
            )
        );

        // Assert
        sent.unwrap();
        assert_eq!(
            received.unwrap().unwrap(),
            PublishDone {
                code: publish_done_code_to_wire(PublishDoneCode::TrackEnded),
                reason: "finished".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn a_subscribe_for_a_published_track_receives_what_the_origin_writes() {
        // Arrange
        let Connected {
            client,
            server,
            _endpoints,
        } = connected_sessions("origin-subscribe").await;
        let routes = SessionRoutes::default();
        routes
            .origin
            .ensure_track("ns", "track", 0, Retention::NewestOnly)
            .await;
        let mut subscriber = server.subscriber();

        // Act
        let payload = tokio::time::timeout(TIMEOUT, async {
            tokio::select! {
                () = routes.route(&client) => panic!("the session ended"),
                payload = async {
                    let subscription = subscriber
                        .subscribe("ns".into(), "track".into(), SubscribeOption::default())
                        .await
                        .unwrap();
                    let mut reader = subscriber.track_reader(&subscription);
                    routes
                        .origin
                        .write(
                            "ns",
                            "track",
                            OutgoingObject::plain(GroupBoundary::Next, Bytes::from_static(b"o0")),
                        )
                        .await
                        .unwrap();
                    reader.next_object().await.unwrap().unwrap().payload
                } => payload,
            }
        })
        .await
        .unwrap();

        // Assert
        assert_eq!(payload, Bytes::from_static(b"o0"));
    }

    #[tokio::test]
    async fn a_subscribe_the_origin_answers_reaches_the_track_s_subscriber_watch() {
        // Arrange
        let Connected {
            client,
            server,
            _endpoints,
        } = connected_sessions("subscriber-watch").await;
        let routes = SessionRoutes::default();
        routes
            .origin
            .ensure_track("ns", "track", 0, Retention::NewestOnly)
            .await;
        let mut subscribers = routes.watch_subscribers("ns", "track");
        let mut other_track = routes.watch_subscribers("ns", "other");
        let mut subscriber = server.subscriber();

        // Act
        tokio::time::timeout(TIMEOUT, async {
            tokio::select! {
                () = routes.route(&client) => panic!("the session ended"),
                () = async {
                    subscriber
                        .subscribe("ns".into(), "track".into(), SubscribeOption::default())
                        .await
                        .unwrap();
                    subscribers.recv().await.unwrap();
                } => {}
            }
        })
        .await
        .unwrap();

        // Assert: a watch on another track hears nothing
        assert!(other_track.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_subscribe_from_the_peer_is_rejected_as_track_does_not_exist() {
        // Arrange
        let Connected {
            client,
            server,
            _endpoints,
        } = connected_sessions("reject-subscribe").await;
        let routes = SessionRoutes::default();

        // Act
        let subscribed = tokio::time::timeout(TIMEOUT, async {
            let mut subscriber = server.subscriber();
            tokio::select! {
                () = routes.route(&client) => panic!("the session ended"),
                subscribed = subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()) => subscribed,
            }
        })
        .await
        .unwrap();

        // Assert
        assert!(matches!(
            subscribed,
            Err(RequestFailure::Rejected {
                code: RequestErrorCode::TrackDoesNotExist,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn a_subscribe_outside_every_broadcast_passes_them_and_is_rejected() {
        // Arrange
        let Connected {
            client,
            server,
            _endpoints,
        } = connected_sessions("routes-broadcast").await;
        let client = std::sync::Arc::new(client);
        let routes = SessionRoutes::default();
        routes.add_broadcast(RelayConnection::with_session(
            client.clone(),
            routes.origin.clone(),
        ));

        // Act
        let subscribed = tokio::time::timeout(TIMEOUT, async {
            let mut subscriber = server.subscriber();
            tokio::select! {
                () = routes.route(&client) => panic!("the session ended"),
                subscribed = subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()) => subscribed,
            }
        })
        .await
        .unwrap();

        // Assert
        assert!(matches!(
            subscribed,
            Err(RequestFailure::Rejected {
                code: RequestErrorCode::TrackDoesNotExist,
                ..
            })
        ));
    }
}

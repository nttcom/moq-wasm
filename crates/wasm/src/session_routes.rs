use std::{cell::RefCell, collections::HashMap};

use moqt::{
    RequestErrorCode, Session, SessionEvent,
    namespace::{NamespaceEvent, NamespaceWatchers},
    publish::Origin,
    wire::publish_done_code_to_wire,
};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishDone {
    pub(crate) code: u64,
    pub(crate) reason: String,
}

/// `origin` answers the peer's SUBSCRIBE, UNSUBSCRIBE, FETCH, FETCH_CANCEL
/// and TRACK_STATUS for the tracks this session publishes; a SUBSCRIBE for
/// any other track, and every PUBLISH, is rejected.
#[derive(Default)]
pub(crate) struct SessionRoutes {
    pub(crate) origin: Origin,
    watchers: NamespaceWatchers,
    publish_done: RefCell<HashMap<u64, oneshot::Sender<PublishDone>>>,
}

impl SessionRoutes {
    pub(crate) fn watch(&self, prefix: Vec<String>) -> mpsc::UnboundedReceiver<NamespaceEvent> {
        self.watchers.watch(prefix)
    }

    /// Fails once the session ends or `forget` drops it.
    pub(crate) fn publish_done(&self, request_id: u64) -> oneshot::Receiver<PublishDone> {
        let (sender, receiver) = oneshot::channel();
        self.publish_done.borrow_mut().insert(request_id, sender);
        receiver
    }

    pub(crate) fn forget(&self, request_id: u64) {
        self.publish_done.borrow_mut().remove(&request_id);
    }

    /// Returns once the session has ended.
    pub(crate) async fn route(&self, session: &Session) {
        while let Ok(event) = session.receive_event().await {
            let Some(event) = self.origin.handle(event).await else {
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
}

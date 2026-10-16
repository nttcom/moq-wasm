mod group_cutover;
pub(crate) mod migrating_track;
mod migration_task;

use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use crate::{
    Endpoint, RequestFailure, Session, SessionEvent, TerminationErrorCode, TransportClose,
    TransportProtocol,
    modules::{
        executor::{BoxFuture, MaybeSend, MaybeSync},
        moqt::domains::migrating_session::{
            migrating_track::{MigratingTrack, TrackSource},
            migration_task::MigrationTask,
        },
    },
    publish::Origin,
    subscribe::SubscribeOption,
};

/// A client session that follows the server's GOAWAY (draft-14 §3.5).
/// It connects to the New Session URI (the current URL when the GOAWAY
/// names none), sends every SUBSCRIBE_NAMESPACE, PUBLISH_NAMESPACE, PUBLISH
/// and SUBSCRIBE made through this type again there, and closes the previous
/// session with NO_ERROR once every subscribed track has moved and the
/// server no longer takes any published track there. Until then `origin()`
/// writes to both sessions (draft-22 §7.6.1). `MigratingTrack`s survive the
/// migration; `receive_event` returns the events of both sessions. A failed
/// migration keeps the current session until the server closes it.
///
/// A SUBSCRIBE sent again continues live from the next group start. Only
/// requests made through this type move: a FETCH or another request sent on
/// `session()` stays on that session.
pub struct MigratingSession {
    shared: Arc<Shared>,
    event_receiver: tokio::sync::Mutex<mpsc::UnboundedReceiver<SessionEvent>>,
    migration_task: MigrationTask,
}

struct Shared {
    connector: Box<dyn SessionConnector>,
    origin: Origin,
    current: std::sync::Mutex<Current>,
    /// The session a migration moved away from, until it is closed.
    previous: std::sync::Mutex<Option<Arc<Session>>>,
    /// Locked while a request is sent and while a migration sends all of
    /// them again, so a request sent during a migration is not skipped.
    requests: tokio::sync::Mutex<Requests>,
    migrating: watch::Sender<bool>,
    closed: watch::Sender<Option<TransportClose>>,
}

#[derive(Clone)]
struct Current {
    session: Arc<Session>,
    url: String,
}

#[derive(Default)]
struct Requests {
    namespaces: Vec<String>,
    published_namespaces: Vec<String>,
    tracks: Vec<TrackRequest>,
}

struct TrackRequest {
    namespace: String,
    name: String,
    option: SubscribeOption,
    source_sender: mpsc::UnboundedSender<TrackSource>,
}

trait SessionConnector: MaybeSend + MaybeSync {
    fn connect(&self, url: String) -> BoxFuture<anyhow::Result<Session>>;
    fn wait_idle(&self) -> BoxFuture<()>;
}

impl<T: TransportProtocol> SessionConnector for Arc<Endpoint<T>> {
    fn connect(&self, url: String) -> BoxFuture<anyhow::Result<Session>> {
        let endpoint = self.clone();
        Box::pin(async move { endpoint.as_ref().connect(&url).await?.await })
    }

    fn wait_idle(&self) -> BoxFuture<()> {
        let endpoint = self.clone();
        Box::pin(async move { endpoint.as_ref().wait_idle().await })
    }
}

impl MigratingSession {
    /// Opens the first session; every later one goes through `endpoint` too.
    pub async fn connect<T: TransportProtocol>(
        endpoint: Endpoint<T>,
        url: &str,
    ) -> anyhow::Result<Self> {
        let connector: Box<dyn SessionConnector> = Box::new(Arc::new(endpoint));
        let session = Arc::new(connector.connect(url.to_string()).await?);
        let shared = Arc::new(Shared {
            connector,
            origin: Origin::default(),
            previous: std::sync::Mutex::new(None),
            current: std::sync::Mutex::new(Current {
                session,
                url: url.to_string(),
            }),
            requests: tokio::sync::Mutex::new(Requests::default()),
            migrating: watch::channel(false).0,
            closed: watch::channel(None).0,
        });
        let (event_sender, event_receiver) = mpsc::unbounded_channel();
        Ok(Self {
            migration_task: MigrationTask::run(shared.clone(), event_sender),
            shared,
            event_receiver: tokio::sync::Mutex::new(event_receiver),
        })
    }

    /// The session requests currently go to. Its events are read by this
    /// type; take them from `receive_event`.
    pub fn session(&self) -> Arc<Session> {
        self.shared.current().session
    }

    /// The tracks this endpoint publishes, on every session that takes them.
    /// Answer the SUBSCRIBE, UNSUBSCRIBE, FETCH, FETCH_CANCEL and
    /// TRACK_STATUS events with it, and send PUBLISH through `publish`.
    pub fn origin(&self) -> &Origin {
        &self.shared.origin
    }

    pub async fn publish_namespace(&self, namespace: String) -> Result<(), RequestFailure> {
        let mut requests = self.shared.requests.lock().await;
        self.session()
            .publisher()
            .publish_namespace(namespace.clone())
            .await?;
        requests.published_namespaces.push(namespace);
        Ok(())
    }

    /// `Origin::publish` on the current session.
    pub async fn publish(&self, namespace: &str, track_name: &str) -> anyhow::Result<()> {
        let _requests = self.shared.requests.lock().await;
        self.shared
            .origin
            .publish(&self.session(), namespace, track_name)
            .await
    }

    pub async fn subscribe_namespace(&self, namespace: String) -> Result<(), RequestFailure> {
        let mut requests = self.shared.requests.lock().await;
        self.session()
            .subscriber()
            .subscribe_namespace(namespace.clone())
            .await?;
        requests.namespaces.push(namespace);
        Ok(())
    }

    pub async fn subscribe(
        &self,
        track_namespace: String,
        track_name: String,
        option: SubscribeOption,
    ) -> Result<MigratingTrack, RequestFailure> {
        let mut requests = self.shared.requests.lock().await;
        let session = self.session();
        let subscription = session
            .subscriber()
            .subscribe(track_namespace.clone(), track_name.clone(), option.clone())
            .await?;
        let (source_sender, source_receiver) = mpsc::unbounded_channel();
        requests
            .tracks
            .retain(|track| !track.source_sender.is_closed());
        requests.tracks.push(TrackRequest {
            namespace: track_namespace,
            name: track_name,
            option,
            source_sender,
        });
        Ok(MigratingTrack::new(
            session,
            subscription,
            source_receiver,
            self.shared.migrating.subscribe(),
        ))
    }

    /// The events of the current session and, until it is closed, of the
    /// previous one. GOAWAY is handled here and not returned;
    /// `SessionEvent::Disconnected` comes once, when the current session
    /// closed and no migration replaced it.
    pub async fn receive_event(&self) -> anyhow::Result<SessionEvent> {
        match self.event_receiver.lock().await.recv().await {
            Some(event) => Ok(event),
            None => anyhow::bail!("the migration task ended"),
        }
    }

    /// Resolves once the current session has closed and no migration
    /// replaced it.
    pub async fn closed(&self) -> TransportClose {
        let mut closed = self.shared.closed.subscribe();
        let close = closed.wait_for(Option::is_some).await;
        close
            .ok()
            .and_then(|close| close.clone())
            .unwrap_or_else(|| TransportClose {
                code: None,
                reason: "the migration task ended".to_string(),
            })
    }

    /// Closes the sessions and waits until their transports have finished
    /// closing, so the close reaches the server before the process exits.
    /// Every `MigratingTrack` must already be dropped, or the wait never
    /// ends.
    pub async fn close(&self) {
        self.migration_task.abort();
        let previous = self.shared.previous().take();
        for session in previous.into_iter().chain([self.session()]) {
            session.close_with_error(TerminationErrorCode::NoError, "");
        }
        self.shared.connector.wait_idle().await;
    }
}

impl Shared {
    fn current(&self) -> Current {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn previous(&self) -> std::sync::MutexGuard<'_, Option<Arc<Session>>> {
        self.previous
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn replace_current(&self, current: Current) {
        *self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = current;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;

    use super::*;
    use crate::{
        Subscription,
        modules::test_support::{
            DISCONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, accept_publish, dual_client, open_subgroup,
            send_payload, spawn_dual_server, subscribed_track_reader,
        },
        object::{ContentExists, StreamDataSenderFactory},
        publish::Retention,
        subscribe::FilterType,
        wire::SubgroupId,
    };

    const NO_EVENT_TIMEOUT: Duration = Duration::from_millis(500);

    async fn connect(port: u16) -> MigratingSession {
        MigratingSession::connect(dual_client(), &format!("moqt://127.0.0.1:{port}"))
            .await
            .unwrap()
    }

    async fn serve_subscribe(server: &Session) -> (StreamDataSenderFactory, FilterType) {
        let SessionEvent::Subscribe(handler) = server.receive_event().await.unwrap() else {
            panic!("expected SUBSCRIBE");
        };
        let track_alias = handler.ok(0, ContentExists::False).await.unwrap();
        let subscription = handler.into_subscription(track_alias);
        (
            server.publisher().create_stream(&subscription),
            subscription.filter_type(),
        )
    }

    async fn subscribe(
        client: &MigratingSession,
        server: &Session,
    ) -> (MigratingTrack, StreamDataSenderFactory) {
        let (track, (sender, _)) = tokio::join!(
            client.subscribe("ns".into(), "track".into(), SubscribeOption::default()),
            serve_subscribe(server)
        );
        (track.unwrap(), sender)
    }

    async fn next_payload(track: &mut MigratingTrack) -> (u64, Bytes) {
        let object = tokio::time::timeout(HANDSHAKE_TIMEOUT, track.next_object())
            .await
            .expect("object within timeout")
            .unwrap()
            .expect("track still open");
        (object.group_id, object.payload)
    }

    fn payload(group_id: u64, payload: &'static [u8]) -> (u64, Bytes) {
        (group_id, Bytes::from_static(payload))
    }

    #[tokio::test]
    async fn go_away_moves_a_track_to_the_named_server_at_a_group_boundary() {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("migrating-track-a");
        let (port_b, accept_b) = spawn_dual_server("migrating-track-b");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let (mut track, sender_a) = subscribe(&client, &server_a).await;
        let mut group_1_a = open_subgroup(&sender_a, 1, SubgroupId::None).await;
        send_payload(&mut group_1_a, b"a1-0").await;
        assert_eq!(next_payload(&mut track).await, payload(1, b"a1-0"));

        // Act
        server_a
            .go_away(format!("moqt://127.0.0.1:{port_b}"))
            .await
            .unwrap();
        let server_b = accept_b.await.unwrap();
        let (sender_b, filter_b) = serve_subscribe(&server_b).await;
        send_payload(&mut group_1_a, b"a1-1").await;
        let previous_tail = next_payload(&mut track).await;
        let mut group_1_b = open_subgroup(&sender_b, 1, SubgroupId::None).await;
        send_payload(&mut group_1_b, b"b1-0").await;
        let mut group_2_b = open_subgroup(&sender_b, 2, SubgroupId::None).await;
        send_payload(&mut group_2_b, b"b2-0").await;
        let cutover = next_payload(&mut track).await;
        send_payload(&mut group_1_a, b"a1-2").await;
        let late_previous_tail = next_payload(&mut track).await;
        let mut group_2_a = open_subgroup(&sender_a, 2, SubgroupId::None).await;
        send_payload(&mut group_2_a, b"a2-0").await;
        send_payload(&mut group_2_b, b"b2-1").await;
        let after_cutover = next_payload(&mut track).await;
        let previous_close = tokio::time::timeout(DISCONNECT_TIMEOUT, server_a.closed()).await;

        // Assert: B's group 1 and A's group 2 are dropped; A is closed with NO_ERROR
        assert_eq!(filter_b, FilterType::NextGroupStart);
        assert_eq!(previous_tail, payload(1, b"a1-1"));
        assert_eq!(cutover, payload(2, b"b2-0"));
        assert_eq!(late_previous_tail, payload(1, b"a1-2"));
        assert_eq!(after_cutover, payload(2, b"b2-1"));
        assert_eq!(
            previous_close.expect("previous session closed").code,
            Some(0)
        );
    }

    async fn accept_namespace(server: &Session) {
        let SessionEvent::PublishNamespace(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_NAMESPACE");
        };
        handler.ok().await.unwrap();
    }

    async fn publish_track(client: &MigratingSession, server: &Session) -> Subscription {
        client
            .origin()
            .ensure_track("ns", "track", 1, Retention::NewestOnly)
            .await;
        let (announced, ()) = tokio::join!(
            client.publish_namespace("ns".into()),
            accept_namespace(server)
        );
        announced.unwrap();
        let (published, subscription) =
            tokio::join!(client.publish("ns", "track"), accept_publish(server));
        published.unwrap();
        subscription
    }

    async fn next_group_payload(server: &Session, subscription: &Subscription) -> Bytes {
        let mut reader = subscribed_track_reader(server, subscription).await;
        tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.next_object())
            .await
            .expect("object within timeout")
            .unwrap()
            .expect("track still open")
            .payload
    }

    #[tokio::test]
    async fn go_away_publishes_on_the_new_server_and_writes_to_both_until_the_old_one_unsubscribes()
    {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("migrating-publish-a");
        let (port_b, accept_b) = spawn_dual_server("migrating-publish-b");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let published_a = publish_track(&client, &server_a).await;
        let session_a = client.session();

        // Act
        server_a
            .go_away(format!("moqt://127.0.0.1:{port_b}"))
            .await
            .unwrap();
        let server_b = accept_b.await.unwrap();
        accept_namespace(&server_b).await;
        let published_b = accept_publish(&server_b).await;
        tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            while Arc::ptr_eq(&client.session(), &session_a) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the session must move to B");
        client
            .origin()
            .write_group("ns", "track", Bytes::from_static(b"both"))
            .await
            .unwrap();
        let on_a = next_group_payload(&server_a, &published_a).await;
        let on_b = next_group_payload(&server_b, &published_b).await;
        let still_open = tokio::time::timeout(NO_EVENT_TIMEOUT, server_a.closed()).await;
        server_a
            .subscriber()
            .unsubscribe(&published_a)
            .await
            .unwrap();
        let SessionEvent::Unsubscribe(unsubscribe) =
            tokio::time::timeout(HANDSHAKE_TIMEOUT, client.receive_event())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("expected UNSUBSCRIBE");
        };
        client.origin().release(&unsubscribe).await;
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, server_a.closed()).await;

        // Assert
        assert_eq!(on_a, Bytes::from_static(b"both"));
        assert_eq!(on_b, Bytes::from_static(b"both"));
        assert!(
            still_open.is_err(),
            "A must stay open while it takes the track"
        );
        assert_eq!(close.expect("A closed once it unsubscribed").code, Some(0));
    }

    #[tokio::test]
    async fn go_away_subscribes_the_namespaces_again_and_forwards_the_new_sessions_events() {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("migrating-namespace-a");
        let (port_b, accept_b) = spawn_dual_server("migrating-namespace-b");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let (subscribed, ()) = tokio::join!(client.subscribe_namespace("room".into()), async {
            let SessionEvent::SubscribeNameSpace(handler) = server_a.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_NAMESPACE");
            };
            handler.ok().await.unwrap();
        });
        subscribed.unwrap();

        // Act
        server_a
            .go_away(format!("moqt://127.0.0.1:{port_b}"))
            .await
            .unwrap();
        let server_b = accept_b.await.unwrap();
        let SessionEvent::SubscribeNameSpace(handler) = server_b.receive_event().await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE on the new session");
        };
        handler.ok().await.unwrap();
        let _pending = server_b
            .publisher()
            .begin_publish_namespace("room/alice".into())
            .await
            .unwrap();
        let event = tokio::time::timeout(HANDSHAKE_TIMEOUT, client.receive_event())
            .await
            .unwrap()
            .unwrap();

        // Assert: the GOAWAY itself is not returned
        assert_eq!(handler.track_namespace_prefix, "room");
        let SessionEvent::PublishNamespace(announced) = event else {
            panic!("expected PUBLISH_NAMESPACE, got {event:?}");
        };
        assert_eq!(announced.track_namespace, "room/alice");
    }

    #[tokio::test]
    async fn a_go_away_to_an_unreachable_server_keeps_the_current_session() {
        // Arrange
        let (port_a, accept_a) = spawn_dual_server("migrating-unreachable");
        let client = connect(port_a).await;
        let server_a = accept_a.await.unwrap();
        let (mut track, sender_a) = subscribe(&client, &server_a).await;

        // Act
        server_a
            .go_away("ftp://relay.invalid".to_string())
            .await
            .unwrap();
        let mut group_1 = open_subgroup(&sender_a, 1, SubgroupId::None).await;
        send_payload(&mut group_1, b"a1-0").await;
        let object = next_payload(&mut track).await;
        let still_open = tokio::time::timeout(NO_EVENT_TIMEOUT, client.closed()).await;
        server_a.close_with_error(TerminationErrorCode::GoawayTimeout, "drained");
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed()).await;
        let event = tokio::time::timeout(DISCONNECT_TIMEOUT, client.receive_event()).await;

        // Assert
        assert_eq!(object, payload(1, b"a1-0"));
        assert!(still_open.is_err(), "the session must stay open");
        assert_eq!(close.expect("closed").code, Some(0x10));
        assert!(matches!(
            event.unwrap().unwrap(),
            SessionEvent::Disconnected()
        ));
    }
}

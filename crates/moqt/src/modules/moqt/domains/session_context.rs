use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, Weak},
};

use tokio::sync::watch;

use crate::{
    SessionEvent,
    modules::moqt::{
        control_plane::{constants::TerminationErrorCode, enums::RequestId},
        data_plane::stream::{
            fetch_data_receiver::FetchDataReceiver, open_subgroups::OpenSubgroups,
        },
        domains::connection::{
            Connection, ObjectSink, SinkReceiver, generation_of, logical_id, wire_part,
        },
        runtime::dispatch::incoming_object::IncomingObject,
    },
    publish::PublishOption,
    subscribe::SubscribeOption,
};

/// The state of a session that outlives its transport connections: the
/// events, the receivers the application takes per track alias and per
/// FETCH (keyed by logical alias and logical request id), and, for a client
/// that follows GOAWAY, the requests a migration sends again.
pub(crate) struct SessionContext {
    connections: std::sync::Mutex<Connections>,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    pub(crate) receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject>>>,
    pub(crate) fetch_receiver_map:
        std::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<FetchDataReceiver>>>,
    /// `None` for a session that does not follow GOAWAY. Held while a
    /// carried request is sent and while a migration sends them all again,
    /// so a request sent during a migration is not skipped.
    pub(crate) carried: Option<tokio::sync::Mutex<CarriedRequests>>,
    /// For a session that follows GOAWAY, the tracks this side PUBLISHed,
    /// keyed by logical request id, with every connection each is on. Read
    /// for every subgroup sent, so not behind `carried`.
    publications: std::sync::Mutex<HashMap<u64, Publication>>,
    /// Bumped whenever a connection stops serving a track.
    served_changed: watch::Sender<()>,
}

pub(crate) struct Publication {
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
    pub(crate) option: PublishOption,
    pub(crate) targets: Vec<PublicationTarget>,
}

pub(crate) struct PublicationTarget {
    pub(crate) connection: Weak<Connection>,
    pub(crate) track_alias: u64,
    pub(crate) request_id: u64,
}

struct Connections {
    current: Arc<Connection>,
    /// The connection a migration moved away from, until it is closed.
    previous: Option<Arc<Connection>>,
}

#[derive(Default)]
pub(crate) struct CarriedRequests {
    pub(crate) namespaces: Vec<String>,
    pub(crate) published_namespaces: Vec<String>,
    /// Keyed by logical request id.
    pub(crate) subscriptions: HashMap<u64, CarriedSubscription>,
}

pub(crate) struct CarriedSubscription {
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
    /// With the Forward state of the last SUBSCRIBE_UPDATE.
    pub(crate) option: SubscribeOption,
}

#[must_use = "dropping this removes the pending fetch; bind it for the lifetime of the request"]
pub(crate) struct PendingFetch {
    session: Arc<SessionContext>,
    connection: Arc<Connection>,
    request_id: RequestId,
    kept: bool,
}

impl PendingFetch {
    /// FETCH_OK arrived: the channel stays for `accept_fetch_receiver`.
    pub(crate) fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for PendingFetch {
    fn drop(&mut self) {
        if !self.kept {
            self.session
                .remove_pending_fetch(&self.connection, self.request_id);
        }
    }
}

impl SessionContext {
    pub(crate) fn new(
        connection: Connection,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        follows_go_away: bool,
    ) -> Self {
        Self {
            connections: std::sync::Mutex::new(Connections {
                current: Arc::new(connection),
                previous: None,
            }),
            event_sender,
            receiver_map: tokio::sync::Mutex::new(HashMap::new()),
            fetch_receiver_map: std::sync::Mutex::new(HashMap::new()),
            carried: follows_go_away.then(|| tokio::sync::Mutex::new(CarriedRequests::default())),
            publications: std::sync::Mutex::default(),
            served_changed: watch::channel(()).0,
        }
    }

    pub(crate) fn follows_go_away(&self) -> bool {
        self.carried.is_some()
    }

    pub(crate) async fn lock_carried(
        &self,
    ) -> Option<tokio::sync::MutexGuard<'_, CarriedRequests>> {
        match &self.carried {
            Some(carried) => Some(carried.lock().await),
            None => None,
        }
    }

    /// The live connection a logical id belongs to, with its wire part.
    pub(crate) fn connection_of(&self, logical_id: u64) -> Option<(Arc<Connection>, u64)> {
        self.live_connections()
            .into_iter()
            .find(|connection| connection.generation == generation_of(logical_id))
            .map(|connection| (connection, wire_part(logical_id)))
    }

    /// Drops `connection` from every publication, once it is closed.
    pub(crate) fn forget_connection(&self, connection: &Connection) {
        for publication in self.publications().values_mut() {
            publication.targets.retain(|target| {
                target
                    .connection
                    .upgrade()
                    .is_some_and(|target| !std::ptr::eq(&*target, connection))
            });
        }
        self.served_changed.send_replace(());
    }

    fn publications(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Publication>> {
        self.publications
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn add_publication(&self, request_id: u64, publication: Publication) {
        self.publications().insert(request_id, publication);
    }

    pub(crate) fn remove_publication(&self, request_id: u64) -> Option<Publication> {
        let publication = self.publications().remove(&request_id);
        self.served_changed.send_replace(());
        publication
    }

    /// The live connections the publication `request_id` is on, with its
    /// wire track alias on each.
    pub(crate) fn publication_targets(&self, request_id: u64) -> Vec<(Arc<Connection>, u64)> {
        self.publications()
            .get(&request_id)
            .map(|publication| {
                publication
                    .targets
                    .iter()
                    .filter_map(|target| Some((target.connection.upgrade()?, target.track_alias)))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn has_publication(&self, request_id: u64) -> bool {
        self.publications().contains_key(&request_id)
    }

    /// What a migration sends again: each publication with its logical id.
    pub(crate) fn publication_requests(&self) -> Vec<(u64, String, String, PublishOption)> {
        self.publications()
            .iter()
            .map(|(request_id, publication)| {
                (
                    *request_id,
                    publication.track_namespace.clone(),
                    publication.track_name.clone(),
                    publication.option.clone(),
                )
            })
            .collect()
    }

    pub(crate) fn add_publication_target(&self, request_id: u64, target: PublicationTarget) {
        if let Some(publication) = self.publications().get_mut(&request_id) {
            publication.targets.push(target);
        }
    }

    /// Drops `connection` from the publication `request_id`. `true` when the
    /// publication is still on another connection.
    pub(crate) fn leave_publication(&self, request_id: u64, connection: &Connection) -> bool {
        let still_published = match self.publications().get_mut(&request_id) {
            Some(publication) => {
                publication.targets.retain(|target| {
                    target
                        .connection
                        .upgrade()
                        .is_some_and(|target| !std::ptr::eq(&*target, connection))
                });
                !publication.targets.is_empty()
            }
            None => false,
        };
        self.served_changed.send_replace(());
        still_published
    }

    /// Whether the peer of `connection` still takes a track this side sends:
    /// a publication on it or a SUBSCRIBE it sent that it has not ended.
    pub(crate) fn serves_on(&self, connection: &Arc<Connection>) -> bool {
        connection.serves_subscriptions()
            || self.publications().values().any(|publication| {
                publication.targets.iter().any(|target| {
                    target
                        .connection
                        .upgrade()
                        .is_some_and(|target| Arc::ptr_eq(&target, connection))
                })
            })
    }

    /// Resolves once the peer of `connection` takes no track this side sends.
    pub(crate) async fn unserved(&self, connection: &Arc<Connection>) {
        let mut changed = self.served_changed.subscribe();
        while self.serves_on(connection) {
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    pub(crate) fn note_served_changed(&self) {
        self.served_changed.send_replace(());
    }

    /// The connection requests are sent on.
    pub(crate) fn connection(&self) -> Arc<Connection> {
        self.connections().current.clone()
    }

    /// The current connection first, then the one being migrated away from.
    pub(crate) fn live_connections(&self) -> Vec<Arc<Connection>> {
        let connections = self.connections();
        std::iter::once(connections.current.clone())
            .chain(connections.previous.clone())
            .collect()
    }

    /// Makes `next` the current connection and the current one the previous.
    pub(crate) fn move_to(&self, next: Arc<Connection>) -> Arc<Connection> {
        let mut connections = self.connections();
        let moved_from = std::mem::replace(&mut connections.current, next);
        connections.previous = Some(moved_from.clone());
        moved_from
    }

    pub(crate) fn forget_previous(&self, connection: &Arc<Connection>) {
        let mut connections = self.connections();
        if connections
            .previous
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, connection))
        {
            connections.previous = None;
        }
    }

    fn connections(&self) -> std::sync::MutexGuard<'_, Connections> {
        self.connections
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Registers the receiver of a subscription accepted on `connection`
    /// and returns its logical track alias, the key of `receiver_map`.
    /// `receiver_map` is updated while the object-sinks lock is held, so the
    /// drain, the sink switch and the receiver registration are atomic.
    pub(crate) async fn register_data_receiver(
        &self,
        connection: &Connection,
        request_id: RequestId,
        track_alias: u64,
    ) -> Result<u64, TerminationErrorCode> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<IncomingObject>();
        let logical_alias = logical_id(connection.generation, track_alias);
        let _sinks = connection
            .install_receiver(
                track_alias,
                SinkReceiver {
                    request_id,
                    sender,
                    logical_alias,
                    newest_group: None,
                    cutover: None,
                    open_subgroups: OpenSubgroups::new(),
                },
            )
            .await?;
        self.receiver_map
            .lock()
            .await
            .insert(logical_alias, receiver);
        Ok(logical_alias)
    }

    pub(crate) async fn cancel_subscription(&self, connection: &Connection, request_id: RequestId) {
        let mut sinks = connection.object_sinks.lock().await;
        let found = sinks.iter().find_map(|(track_alias, sink)| match sink {
            ObjectSink::Receiver(receiver) if receiver.request_id == request_id => {
                Some((*track_alias, receiver.logical_alias))
            }
            _ => None,
        });
        if let Some((track_alias, logical_alias)) = found {
            sinks.insert(track_alias, ObjectSink::Cancelled);
            self.receiver_map.lock().await.remove(&logical_alias);
        }
    }

    pub(crate) async fn cancel_unclaimed_track_alias(
        &self,
        connection: &Connection,
        track_alias: u64,
    ) {
        let mut sinks = connection.object_sinks.lock().await;
        if !sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            sinks.insert(track_alias, ObjectSink::Cancelled);
        }
    }

    /// Call before sending the FETCH on `connection`: its data stream can
    /// arrive before FETCH_OK.
    pub(crate) fn register_pending_fetch(
        self: &Arc<Self>,
        connection: &Arc<Connection>,
        request_id: RequestId,
    ) -> PendingFetch {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<FetchDataReceiver>();
        connection
            .fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .insert(request_id, sender);
        self.fetch_receiver_map
            .lock()
            .expect("fetch_receiver_map poisoned")
            .insert(logical_id(connection.generation, request_id), receiver);
        PendingFetch {
            session: self.clone(),
            connection: connection.clone(),
            request_id,
            kept: false,
        }
    }

    pub(crate) fn remove_pending_fetch(&self, connection: &Connection, request_id: RequestId) {
        connection
            .fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .remove(&request_id);
        self.fetch_receiver_map
            .lock()
            .expect("fetch_receiver_map poisoned")
            .remove(&logical_id(connection.generation, request_id));
    }

    /// A close for a protocol violation is reported at once as
    /// `SessionEvent::ProtocolViolation`; any other close is reported as
    /// `SessionEvent::Disconnected` once the transport has closed, like a
    /// close by the peer.
    pub(crate) fn close_with_error(&self, code: TerminationErrorCode, reason: &str) {
        self.close_connection(&self.connection(), code, reason);
    }

    /// Like `close_with_error`, for the connection a message arrived on.
    pub(crate) fn close_connection(
        &self,
        connection: &Connection,
        code: TerminationErrorCode,
        reason: &str,
    ) {
        if code == TerminationErrorCode::ProtocolViolation
            && let Err(error) = connection
                .event_sender
                .send(SessionEvent::ProtocolViolation())
        {
            tracing::error!(?error, "failed to send protocol violation event");
        }
        connection.close(code, reason);
    }
}

impl Drop for SessionContext {
    fn drop(&mut self) {
        tracing::debug!("SessionContext dropped.");
    }
}

impl fmt::Debug for SessionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionContext").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::modules::moqt::domains::connection::IncomingObjectNotification;
    use crate::modules::{
        moqt::runtime::dispatch::incoming_object::IncomingObject,
        test_support::{
            datagram_object, notify_datagram, register_and_take_data_receiver,
            spawn_connected_dual_sessions, subscription,
        },
    };

    const REQUEST_ID: u64 = 1;
    const OTHER_REQUEST_ID: u64 = 3;
    const TRACK_ALIAS: u64 = 5;

    #[tokio::test]
    async fn concurrent_callers_get_distinct_request_ids() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("request-id-concurrent").await;
        let context = server.subscriber().session;

        // Act
        let request_ids: Vec<u64> = std::thread::scope(|scope| {
            let callers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..1_000)
                            .map(|_| context.connection().control.next_request_id())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            callers
                .into_iter()
                .flat_map(|caller| caller.join().unwrap())
                .collect()
        });

        // Assert
        let distinct: HashSet<u64> = request_ids.iter().copied().collect();
        assert_eq!(distinct.len(), request_ids.len());
    }

    #[tokio::test]
    async fn objects_buffered_before_registration_are_delivered_to_the_receiver() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-early-object").await;
        let context = server.subscriber().session;
        notify_datagram(&context, TRACK_ALIAS).await;

        // Act
        let mut receiver = register_and_take_data_receiver(&context, REQUEST_ID, TRACK_ALIAS).await;

        // Assert
        let received = receiver.try_recv();
        assert!(
            matches!(&received, Ok(IncomingObject::Datagram(object)) if *object == datagram_object()),
            "{received:?}"
        );
    }

    #[tokio::test]
    async fn unsubscribe_drops_the_unaccepted_receiver_and_discards_later_objects() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-unsubscribe-unaccepted").await;
        let context = server.subscriber().session;
        context
            .register_data_receiver(&context.connection(), REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();

        // Act
        server
            .subscriber()
            .unsubscribe(&subscription(REQUEST_ID, TRACK_ALIAS))
            .await
            .unwrap();
        let notification = notify_datagram(&context, TRACK_ALIAS).await;

        // Assert
        assert!(!context.receiver_map.lock().await.contains_key(&TRACK_ALIAS));
        assert!(
            matches!(notification, IncomingObjectNotification::Discarded),
            "{notification:?}"
        );
    }

    #[tokio::test]
    async fn unsubscribe_ends_the_accepted_receiver() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-unsubscribe-accepted").await;
        let context = server.subscriber().session;
        let mut receiver = register_and_take_data_receiver(&context, REQUEST_ID, TRACK_ALIAS).await;

        // Act
        server
            .subscriber()
            .unsubscribe(&subscription(REQUEST_ID, TRACK_ALIAS))
            .await
            .unwrap();

        // Assert
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn track_alias_of_a_cancelled_subscription_can_be_reused() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-reuse-alias").await;
        let context = server.subscriber().session;
        context
            .register_data_receiver(&context.connection(), REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();
        context
            .cancel_subscription(&context.connection(), REQUEST_ID)
            .await;

        // Act
        let registration = context
            .register_data_receiver(&context.connection(), OTHER_REQUEST_ID, TRACK_ALIAS)
            .await;

        // Assert
        assert!(registration.is_ok());
    }
}

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::{Arc, Weak},
};

use tokio::sync::watch;

use crate::modules::extensions::mutex::lock;
use crate::{
    modules::{
        executor,
        moqt::{
            control_plane::{
                constants::TerminationErrorCode, enums::RequestId,
                namespace_interest::NamespaceInterest,
            },
            data_plane::{
                object_ledger::ObjectLedger,
                stream::{fetch_data_receiver::FetchDataReceiver, open_subgroups::OpenSubgroups},
            },
            domains::{
                connection::{Connection, SinkReceiver, generation_of, logical_id, wire_part},
                request_failure::RequestFailure,
                session_events::SessionEventSender,
            },
            runtime::dispatch::incoming_object::{IncomingObjectReceiver, incoming_object_channel},
            version::Version,
        },
        transport::transport_stats::TransferredBytes,
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
    pub(crate) event_sender: SessionEventSender,
    pub(crate) receiver_map: tokio::sync::Mutex<HashMap<u64, DataChannel>>,
    pub(crate) fetch_receiver_map: std::sync::Mutex<HashMap<u64, FetchChannel>>,
    /// The ledger of each track a subscription or FETCH is reading, keyed by
    /// namespace and name; the readers hold it, so it ends with the last one.
    ledgers: std::sync::Mutex<HashMap<(String, String), Weak<ObjectLedger>>>,
    /// `None` for a session that does not follow GOAWAY. A new request holds the
    /// read lock from before it is sent until it is recorded (a PUBLISH in
    /// `publications`) or fails; a migration holds the write lock while it sends
    /// every carried request again, so none is skipped. Requests edit the record
    /// through the inner mutex, so they do not wait for one another's answers.
    pub(crate) carried: Option<tokio::sync::RwLock<std::sync::Mutex<CarriedRequests>>>,
    /// The namespaces this side sent PUBLISH_NAMESPACE for and has not
    /// withdrawn, keyed as `CarriedRequests::published_namespaces`. Only
    /// `PublishedNamespaceRecord` and `withdraw_published_namespace` change
    /// it, each together with the carried map.
    published_namespaces: std::sync::Mutex<BTreeMap<u64, String>>,
    /// For a session that follows GOAWAY, the tracks this side PUBLISHed,
    /// keyed by logical request id, with every connection each is on. Read
    /// for every subgroup sent, so not behind `carried`.
    publications: std::sync::Mutex<HashMap<u64, Publication>>,
    /// Wakes `unserved` whenever a connection stops serving a track.
    served_changed: watch::Sender<()>,
    /// Changes whenever a migration makes another connection current, and
    /// turns `true` once the session will not move again.
    moves: watch::Sender<bool>,
}

/// The objects of a subscription, until the application takes them.
pub(crate) struct DataChannel {
    pub(crate) receiver: IncomingObjectReceiver,
    /// The version of the connection the subscription was accepted on.
    pub(crate) version: Version,
}

/// The response stream of a FETCH, until the application takes it. It
/// holds no ledger, so a response never taken keeps none alive; taking it
/// joins the track's ledger.
pub(crate) struct FetchChannel {
    pub(crate) receiver: tokio::sync::mpsc::UnboundedReceiver<FetchDataReceiver>,
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
}

pub(crate) struct Publication {
    pub(crate) track_namespace: String,
    pub(crate) track_name: String,
    pub(crate) option: PublishOption,
    pub(crate) targets: Vec<PublicationTarget>,
}

impl Publication {
    /// `true` when the publication is still on another connection.
    fn leave(&mut self, connection: &Connection) -> bool {
        self.targets.retain(|target| {
            target
                .connection
                .upgrade()
                .is_some_and(|target| !std::ptr::eq(&*target, connection))
        });
        !self.targets.is_empty()
    }
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
    /// What the connections that left the session carried.
    retired: TransferredBytes,
}

impl Connections {
    fn retire(&mut self, connection: &Connection) {
        self.retired = self.retired.add(connection.transport.stats());
    }
}

#[derive(Default)]
pub(crate) struct CarriedRequests {
    /// Keyed by the logical id of the request that sent the namespace, so a
    /// failed request removes only its own record, not another request's for
    /// the same namespace.
    pub(crate) namespaces: BTreeMap<u64, (String, NamespaceInterest)>,
    /// Keyed as `namespaces`; changed only with
    /// `SessionContext::published_namespaces`.
    published_namespaces: BTreeMap<u64, String>,
    /// Keyed by logical request id.
    pub(crate) subscriptions: HashMap<u64, CarriedSubscription>,
}

/// Keeps a migration from starting while it is held.
pub(crate) struct CarriedGuard<'a>(
    tokio::sync::RwLockReadGuard<'a, std::sync::Mutex<CarriedRequests>>,
);

impl CarriedRequests {
    /// The namespaces a migration announces again, by logical request id.
    pub(crate) fn published_namespaces(&self) -> &BTreeMap<u64, String> {
        &self.published_namespaces
    }
}

impl CarriedGuard<'_> {
    pub(crate) fn update<T>(&self, update: impl FnOnce(&mut CarriedRequests) -> T) -> T {
        update(&mut lock(&self.0))
    }
}

/// The entries one PUBLISH_NAMESPACE added to
/// `SessionContext::published_namespaces` and
/// `CarriedRequests::published_namespaces`, which `forget` removes. Dropped
/// before `keep` or `forget`, it forgets them too, in a spawned task.
pub(crate) struct PublishedNamespaceRecord {
    session: Arc<SessionContext>,
    request_id: u64,
    /// `keep` or `forget` settled the entries.
    settled: bool,
}

impl PublishedNamespaceRecord {
    /// Records the namespace of the PUBLISH_NAMESPACE `request_id` in both
    /// maps. `carried` is the read lock the request was sent under, `None` for
    /// a session that does not follow GOAWAY.
    pub(crate) fn insert(
        session: &Arc<SessionContext>,
        carried: Option<&CarriedGuard>,
        request_id: u64,
        namespace: String,
    ) -> Self {
        lock(&session.published_namespaces).insert(request_id, namespace.clone());
        if let Some(carried) = carried {
            carried.update(|carried| carried.published_namespaces.insert(request_id, namespace));
        }
        Self {
            session: session.clone(),
            request_id,
            settled: false,
        }
    }

    /// The entries stay: the namespace was accepted, or its connection was
    /// lost and a migration announces it again.
    pub(crate) fn keep(mut self) {
        self.settled = true;
    }

    pub(crate) async fn forget(mut self) {
        self.settled = true;
        lock(&self.session.published_namespaces).remove(&self.request_id);
        self.session.forget_carried_namespace(self.request_id).await;
    }
}

impl Drop for PublishedNamespaceRecord {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        lock(&self.session.published_namespaces).remove(&self.request_id);
        let session = self.session.clone();
        let request_id = self.request_id;
        executor::try_spawn("forget a dropped PUBLISH_NAMESPACE", async move {
            session.forget_carried_namespace(request_id).await;
        });
    }
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
    // On wasm32 the browser transport is !Send, but the context is shared with
    // the native build, where the tasks run on a multi-threaded executor.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(crate) fn new(
        connection: Connection,
        event_sender: SessionEventSender,
        follows_go_away: bool,
    ) -> Self {
        Self {
            connections: std::sync::Mutex::new(Connections {
                current: Arc::new(connection),
                previous: None,
                retired: TransferredBytes::default(),
            }),
            event_sender,
            receiver_map: tokio::sync::Mutex::new(HashMap::new()),
            fetch_receiver_map: std::sync::Mutex::new(HashMap::new()),
            ledgers: std::sync::Mutex::default(),
            carried: follows_go_away.then(tokio::sync::RwLock::default),
            published_namespaces: std::sync::Mutex::default(),
            publications: std::sync::Mutex::default(),
            served_changed: watch::channel(()).0,
            moves: watch::channel(false).0,
        }
    }

    /// The ledger of the track `track_namespace` / `track_name`, shared by
    /// every subscription and FETCH of it the session reads.
    pub(crate) fn object_ledger(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Arc<ObjectLedger> {
        let mut ledgers = lock(&self.ledgers);
        let key = (track_namespace.to_string(), track_name.to_string());
        if let Some(ledger) = ledgers.get(&key).and_then(Weak::upgrade) {
            return ledger;
        }
        ledgers.retain(|_, ledger| ledger.strong_count() > 0);
        let ledger = Arc::new(ObjectLedger::new());
        ledgers.insert(key, Arc::downgrade(&ledger));
        ledger
    }

    /// Whether a receiver still holds the ledger of the track.
    #[cfg(test)]
    pub(crate) fn holds_ledger(&self, track_namespace: &str, track_name: &str) -> bool {
        self.ledgers
            .lock()
            .unwrap()
            .get(&(track_namespace.to_string(), track_name.to_string()))
            .is_some_and(|ledger| ledger.strong_count() > 0)
    }

    pub(crate) fn follows_go_away(&self) -> bool {
        self.carried.is_some()
    }

    pub(crate) fn publishes_namespace(&self, namespace: &str) -> bool {
        lock(&self.published_namespaces)
            .values()
            .any(|published| published == namespace)
    }

    /// Removes every record of `namespace` from both maps, awaiting the
    /// `carried` read lock between the two.
    pub(crate) async fn withdraw_published_namespace(&self, namespace: &str) {
        lock(&self.published_namespaces).retain(|_, published| published != namespace);
        if let Some(carried) = self.read_carried().await {
            carried.update(|carried| {
                carried
                    .published_namespaces
                    .retain(|_, carried| carried != namespace)
            });
        }
    }

    async fn forget_carried_namespace(&self, request_id: u64) {
        if let Some(carried) = self.read_carried().await {
            carried.update(|carried| carried.published_namespaces.remove(&request_id));
        }
    }

    pub(crate) async fn read_carried(&self) -> Option<CarriedGuard<'_>> {
        Some(CarriedGuard(self.carried.as_ref()?.read().await))
    }

    /// The live connection a logical id belongs to, with its wire part.
    pub(crate) fn connection_of(&self, logical_id: u64) -> Option<(Arc<Connection>, u64)> {
        self.live_connections()
            .into_iter()
            .find(|connection| connection.generation == generation_of(logical_id))
            .map(|connection| (connection, wire_part(logical_id)))
    }

    /// Drops the closed `connection` from every publication and removes each
    /// publication left on no connection; returns the removed ids.
    pub(crate) fn forget_connection(&self, connection: &Connection) -> Vec<u64> {
        let mut ended = Vec::new();
        lock(&self.publications).retain(|request_id, publication| {
            let still_published = publication.leave(connection);
            if !still_published {
                ended.push(*request_id);
            }
            still_published
        });
        self.served_changed.send_replace(());
        ended
    }

    pub(crate) fn add_publication(&self, request_id: u64, publication: Publication) {
        lock(&self.publications).insert(request_id, publication);
    }

    pub(crate) fn remove_publication(&self, request_id: u64) -> Option<Publication> {
        let publication = lock(&self.publications).remove(&request_id);
        self.served_changed.send_replace(());
        publication
    }

    /// The live connections the publication `request_id` is on, with its
    /// wire track alias on each.
    pub(crate) fn publication_targets(&self, request_id: u64) -> Vec<(Arc<Connection>, u64)> {
        lock(&self.publications)
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
        lock(&self.publications).contains_key(&request_id)
    }

    /// What a migration sends again: each publication with its logical id.
    pub(crate) fn publication_requests(&self) -> Vec<(u64, String, String, PublishOption)> {
        lock(&self.publications)
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
        if let Some(publication) = lock(&self.publications).get_mut(&request_id) {
            publication.targets.push(target);
        }
    }

    /// Drops `connection` from the publication `request_id`, and the
    /// publication once it is on no connection. `true` when it is still on
    /// another one.
    pub(crate) fn leave_publication(&self, request_id: u64, connection: &Connection) -> bool {
        let mut publications = lock(&self.publications);
        let still_published = publications
            .get_mut(&request_id)
            .is_some_and(|publication| publication.leave(connection));
        if !still_published {
            publications.remove(&request_id);
        }
        drop(publications);
        self.served_changed.send_replace(());
        still_published
    }

    /// Whether the peer of `connection` still takes a track this side sends:
    /// a publication on it or a SUBSCRIBE it sent that it has not ended.
    pub(crate) fn serves_on(&self, connection: &Arc<Connection>) -> bool {
        connection.serves_subscriptions()
            || lock(&self.publications).values().any(|publication| {
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
        lock(&self.connections).current.clone()
    }

    /// The current connection first, then the one being migrated away from.
    pub(crate) fn live_connections(&self) -> Vec<Arc<Connection>> {
        let connections = lock(&self.connections);
        std::iter::once(connections.current.clone())
            .chain(connections.previous.clone())
            .collect()
    }

    /// Makes `next` the current connection and the current one the previous.
    pub(crate) fn move_to(&self, next: Arc<Connection>) -> Arc<Connection> {
        let mut connections = lock(&self.connections);
        let moved_from = std::mem::replace(&mut connections.current, next);
        if let Some(older) = connections.previous.replace(moved_from.clone()) {
            connections.retire(&older);
        }
        drop(connections);
        self.moves.send_modify(|_| {});
        moved_from
    }

    pub(crate) fn end_moves(&self) {
        self.moves.send_replace(true);
    }

    /// `Ok` when a migration has replaced the lost `connection`, so the request
    /// is sent again on the current one; otherwise `error`.
    /// Drops `carried`, the lock the request was sent under, first: the
    /// migration takes that lock to send the carried requests.
    pub(crate) async fn resend_after<G>(
        &self,
        carried: G,
        connection: &Arc<Connection>,
        error: RequestFailure,
    ) -> Result<(), RequestFailure> {
        drop(carried);
        if self.moved_after_loss(connection, &error).await {
            Ok(())
        } else {
            Err(error)
        }
    }

    /// Waits until a migration replaces `connection`; `false` when `error` does
    /// not mean `connection` was lost, or the session ends on it.
    pub(crate) async fn moved_after_loss(
        &self,
        connection: &Arc<Connection>,
        error: &RequestFailure,
    ) -> bool {
        if !self.follows_go_away()
            || !matches!(
                error,
                RequestFailure::SessionClosed | RequestFailure::Send(_)
            )
        {
            return false;
        }
        self.moved_past(connection.generation).await
    }

    /// `true` once the current connection is later than `generation`; `false`
    /// once the session will not move again.
    pub(crate) async fn moved_past(&self, generation: u64) -> bool {
        let mut moves = self.moves.subscribe();
        loop {
            let ended = *moves.borrow_and_update();
            if self.connection().generation > generation {
                return true;
            }
            if ended || moves.changed().await.is_err() {
                return false;
            }
        }
    }

    pub(crate) fn forget_previous(&self, connection: &Arc<Connection>) {
        let mut connections = lock(&self.connections);
        if connections
            .previous
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, connection))
        {
            connections.previous = None;
            connections.retire(connection);
        }
    }

    pub(crate) fn transferred_bytes(&self) -> TransferredBytes {
        let connections = lock(&self.connections);
        std::iter::once(&connections.current)
            .chain(&connections.previous)
            .fold(connections.retired, |total, connection| {
                total.add(connection.transport.stats())
            })
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
        let (sender, receiver) = incoming_object_channel();
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
                    default_publisher_priority: connection
                        .control
                        .default_publisher_priority(request_id),
                    arrived_streams: watch::Sender::new(0),
                },
            )
            .await?;
        self.receiver_map.lock().await.insert(
            logical_alias,
            DataChannel {
                receiver,
                version: connection.control.version(),
            },
        );
        Ok(logical_alias)
    }

    /// Keeps the record when `connection` is the previous connection and the
    /// current one carries the subscription: that PUBLISH_DONE only reports
    /// the server a migration left.
    pub(crate) async fn forget_ended_subscription(
        &self,
        connection: &Arc<Connection>,
        request_id: RequestId,
    ) {
        let Some(carried) = self.read_carried().await else {
            return;
        };
        let logical_id = connection.logical_request_id(request_id);
        let current = self.connection();
        if Arc::ptr_eq(&current, connection) || current.wire_request_id(logical_id).is_none() {
            carried.update(|carried| carried.subscriptions.remove(&logical_id));
        }
    }

    /// Ends the subscription `request_id` its PUBLISH_DONE ended: its sink is
    /// cancelled, which drops the sender of its `receiver_map` entry. An entry
    /// the application has not claimed stays, so it still reads the objects
    /// queued there and then the end; it is removed when the application
    /// claims it, or dropped with the session.
    pub(crate) async fn end_subscription(&self, connection: &Connection, request_id: RequestId) {
        connection
            .object_sinks
            .lock()
            .await
            .cancel_subscription(request_id, executor::now());
    }

    pub(crate) async fn cancel_subscription(&self, connection: &Connection, request_id: RequestId) {
        let mut sinks = connection.object_sinks.lock().await;
        if let Some(logical_alias) = sinks.cancel_subscription(request_id, executor::now()) {
            self.receiver_map.lock().await.remove(&logical_alias);
        }
    }

    pub(crate) async fn cancel_unclaimed_track_alias(
        &self,
        connection: &Connection,
        track_alias: u64,
    ) {
        connection
            .object_sinks
            .lock()
            .await
            .cancel_unclaimed(track_alias, executor::now());
    }

    /// Call before sending the FETCH on `connection`: its data stream can
    /// arrive before FETCH_OK.
    pub(crate) fn register_pending_fetch(
        self: &Arc<Self>,
        connection: &Arc<Connection>,
        request_id: RequestId,
        track_namespace: String,
        track_name: String,
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
            .insert(
                logical_id(connection.generation, request_id),
                FetchChannel {
                    receiver,
                    track_namespace,
                    track_name,
                },
            );
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

    pub(crate) async fn unacknowledged_streams_acknowledged(&self) {
        let connections = self.live_connections();
        futures::future::join_all(
            connections
                .iter()
                .map(|connection| connection.unacknowledged_streams.acknowledged()),
        )
        .await;
    }

    /// `Connection::close_with_error` on the session's current connection.
    pub(crate) fn close_with_error(&self, code: TerminationErrorCode, reason: &str) {
        self.connection().close_with_error(code, reason);
    }

    /// Resolves once the application has read enough events for one more
    /// from `connection`. A task awaits it before it queues an event that a
    /// message of the peer raises, so a peer that sends faster than the
    /// application reads is blocked by the transport's flow control (draft-14
    /// §12.1, draft-18 §13.5).
    pub(crate) async fn room_for_event(&self, connection: &Connection) {
        // A session that follows GOAWAY reads `connection`'s events into its own.
        connection.event_sender.has_room().await;
        self.event_sender.has_room().await;
    }
}

impl Drop for SessionContext {
    fn drop(&mut self) {
        tracing::debug!("SessionContext dropped.");
        // Streams the application still holds keep a connection open after
        // its control stream finishes, which both drafts treat as a
        // PROTOCOL_VIOLATION (draft-14 §3.3, draft-18 §3.3); the session ends
        // here instead.
        for connection in self.live_connections() {
            connection.close(TerminationErrorCode::NoError, "");
        }
    }
}

impl fmt::Debug for SessionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionContext").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use crate::SessionEvent;
    use crate::modules::moqt::domains::connection::IncomingObjectNotification;
    use crate::modules::{
        moqt::runtime::dispatch::incoming_object::IncomingObject,
        test_support::{
            DRAFTS, connected_sessions, datagram_object, next_event, notify_datagram,
            register_and_take_data_receiver, spawn_connected_native_sessions, subscription,
        },
    };

    const REQUEST_ID: u64 = 1;
    const OTHER_REQUEST_ID: u64 = 3;
    const TRACK_ALIAS: u64 = 5;

    #[tokio::test]
    async fn concurrent_callers_get_distinct_request_ids() {
        for version in DRAFTS {
            // Arrange
            let (_client, server) = connected_sessions("request-id-concurrent", version).await;
            let context = server.subscriber().session;

            // Act
            let request_ids: Vec<u64> = futures::future::join_all((0..8).map(|_| async {
                let mut ids = Vec::new();
                for _ in 0..50 {
                    ids.push(context.connection().control.take_request_id().await);
                }
                ids
            }))
            .await
            .into_iter()
            .flatten()
            .collect();

            // Assert
            let distinct: std::collections::HashSet<u64> = request_ids.iter().copied().collect();
            assert_eq!(distinct.len(), request_ids.len());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_requests_reach_the_peer_in_increasing_request_id_order() {
        // Arrange
        const REQUESTS: u64 = 32;
        let (client, server) = spawn_connected_native_sessions("request-id-concurrent").await;

        // Act
        let requests: Vec<_> = (0..REQUESTS)
            .map(|_| {
                let subscriber = client.subscriber();
                tokio::spawn(async move {
                    subscriber
                        .track_status("ns".into(), "track".into(), vec![])
                        .await
                })
            })
            .collect();
        let mut request_ids = Vec::new();
        for _ in 0..REQUESTS {
            let SessionEvent::TrackStatus(handler) = next_event(&server).await else {
                panic!("expected TRACK_STATUS from the client");
            };
            request_ids.push(handler.request_id());
        }

        // Assert
        let expected: Vec<u64> = (0..REQUESTS).map(|i| i * 2).collect();
        assert_eq!(request_ids, expected);
        requests.iter().for_each(|request| request.abort());
    }

    #[tokio::test]
    async fn objects_buffered_before_registration_are_delivered_to_the_receiver() {
        for version in DRAFTS {
            // Arrange
            let (_client, server) = connected_sessions("sinks-early-object", version).await;
            let context = server.subscriber().session;
            notify_datagram(&context, TRACK_ALIAS).await;

            // Act
            let mut receiver =
                register_and_take_data_receiver(&context, REQUEST_ID, TRACK_ALIAS).await;

            // Assert
            let received = receiver.try_recv();
            assert!(
                matches!(&received, Ok(IncomingObject::Datagram(datagram)) if datagram.object == datagram_object()),
                "{received:?}"
            );
        }
    }

    #[tokio::test]
    async fn unsubscribe_drops_the_unaccepted_receiver_and_discards_later_objects() {
        for version in DRAFTS {
            // Arrange
            let (_client, server) =
                connected_sessions("sinks-unsubscribe-unaccepted", version).await;
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
    }

    #[tokio::test]
    async fn unsubscribe_ends_the_accepted_receiver() {
        for version in DRAFTS {
            // Arrange
            let (_client, server) = connected_sessions("sinks-unsubscribe-accepted", version).await;
            let context = server.subscriber().session;
            let mut receiver =
                register_and_take_data_receiver(&context, REQUEST_ID, TRACK_ALIAS).await;

            // Act
            server
                .subscriber()
                .unsubscribe(&subscription(REQUEST_ID, TRACK_ALIAS))
                .await
                .unwrap();

            // Assert
            assert!(receiver.recv().await.is_none());
        }
    }

    #[tokio::test]
    async fn track_alias_of_a_cancelled_subscription_can_be_reused() {
        for version in DRAFTS {
            // Arrange
            let (_client, server) = connected_sessions("sinks-reuse-alias", version).await;
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
}

use std::{
    collections::{HashMap, HashSet, VecDeque, hash_map::Entry},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::sync::{mpsc, oneshot};

use crate::{
    SessionEvent,
    modules::{
        executor,
        moqt::{
            control_plane::{constants::TerminationErrorCode, enums::RequestId},
            data_plane::stream::{
                fetch_data_receiver::FetchDataReceiver, open_subgroups::OpenSubgroups,
            },
            domains::group_cutover::GroupCutover,
            runtime::dispatch::incoming_object::{
                IncomingObject, IncomingObjectSendError, IncomingObjectSender,
            },
            version::ControlChannel,
        },
        transport::transport_connection::BoxedConnection,
    },
};

/// draft-14 §10.3, §10.4.2: the objects of an unknown track alias are
/// buffered for a brief period, until SUBSCRIBE_OK or an accepted PUBLISH
/// establishes the alias. §10.1: a cancelled subscription's alias discards
/// late objects for the same period; after it, they are buffered as unknown.
const UNKNOWN_TRACK_ALIAS_PERIOD: Duration = Duration::from_secs(10);
const MAX_PENDING_OBJECTS_PER_TRACK_ALIAS: usize = 256;
/// The objects buffered across all unknown track aliases of a connection:
/// the peer picks the aliases, and datagrams take no flow control.
const MAX_PENDING_OBJECTS: usize = 1024;
/// A logical id is `generation << GENERATION_SHIFT | wire id`: the first
/// connection's ids are their own logical ids, and the ids of a later
/// connection never meet those of another in the session's maps. Request
/// ids stay far below 2^56 (a connection would need 2^55 requests to get
/// there), and so do the track aliases the relay assigns, from 0 up.
const GENERATION_SHIFT: u32 = 56;

pub(crate) fn logical_id(generation: u64, wire_id: u64) -> u64 {
    (generation << GENERATION_SHIFT) | wire_id
}

pub(crate) fn generation_of(logical_id: u64) -> u64 {
    logical_id >> GENERATION_SHIFT
}

pub(crate) fn wire_part(logical_id: u64) -> u64 {
    logical_id & ((1 << GENERATION_SHIFT) - 1)
}

/// One transport connection of a session and the state draft-14 scopes to
/// it: the control stream and its request ids (§9.1), the track aliases this
/// side assigns, whether the peer sent GOAWAY (§9.4), the objects arriving
/// per track alias and the FETCH streams arriving per request id.
pub(crate) struct Connection {
    pub(crate) transport: BoxedConnection,
    pub(crate) control: ControlChannel,
    /// 0 for the session's first connection, then one more per migration.
    pub(crate) generation: u64,
    pub(crate) event_sender: mpsc::UnboundedSender<SessionEvent>,
    is_server: bool,
    go_away_received: AtomicBool,
    closed_locally: AtomicBool,
    track_alias: AtomicU64,
    /// Requests of earlier connections sent again here: logical id ↔ wire id.
    carried: std::sync::Mutex<CarriedIds>,
    /// The logical ids of the SUBSCRIBEs the peer sent here that this side
    /// accepted and neither side has ended. A session that follows GOAWAY
    /// closes a previous connection only once this set is empty.
    served: std::sync::Mutex<HashSet<u64>>,
    pub(crate) object_sinks: tokio::sync::Mutex<ObjectSinks>,
    pub(crate) fetch_notification_map:
        std::sync::Mutex<HashMap<RequestId, mpsc::UnboundedSender<FetchDataReceiver>>>,
}

#[derive(Default)]
struct CarriedIds {
    wire_by_logical: HashMap<u64, u64>,
    logical_by_wire: HashMap<u64, u64>,
}

enum ObjectSink {
    /// Buffers objects that arrive before the receiver is registered.
    Buffer {
        objects: VecDeque<IncomingObject>,
        /// `executor::now()` when the first object arrived.
        created_at: Duration,
    },
    Receiver(SinkReceiver),
    /// Draft-14 §10.1: objects of a cancelled subscription can still arrive
    /// and are discarded instead of being buffered as an unknown track alias.
    /// Kept until a new subscription reuses the track alias or
    /// `UNKNOWN_TRACK_ALIAS_PERIOD` ends.
    Cancelled {
        /// `executor::now()` when the track alias was cancelled.
        created_at: Duration,
    },
}

pub(crate) struct SinkReceiver {
    /// The subscription's wire request id on this connection.
    pub(crate) request_id: RequestId,
    pub(crate) sender: IncomingObjectSender,
    /// The track alias the application knows; set on every stream it gets.
    pub(crate) logical_alias: u64,
    /// The newest group handed to the application, where a cutover starts.
    pub(crate) newest_group: Option<u64>,
    /// Set while the subscription is read on two connections.
    pub(crate) cutover: Option<SinkCutover>,
    pub(crate) open_subgroups: OpenSubgroups,
}

/// What the sink on the new connection takes from the previous one when a
/// subscription is read on two connections.
pub(crate) struct CutoverStart {
    /// The track alias the application knows.
    pub(crate) logical_alias: u64,
    pub(crate) sender: IncomingObjectSender,
    pub(crate) shared: Arc<std::sync::Mutex<SharedCutover>>,
    pub(crate) previous_open_subgroups: OpenSubgroups,
}

/// One side of a subscription read on two connections during a migration.
pub(crate) struct SinkCutover {
    pub(crate) shared: Arc<std::sync::Mutex<SharedCutover>>,
    pub(crate) previous: bool,
}

pub(crate) struct SharedCutover {
    pub(crate) cutover: GroupCutover,
    /// Dropped once the cutover group is fixed.
    pub(crate) cut_over: Option<oneshot::Sender<()>>,
}

/// A subgroup stream carries one group and is taken as starting at the
/// group's first object: a subscription sent again starts at the next group
/// start.
fn location_of(object: &IncomingObject) -> (u64, u64) {
    match object {
        IncomingObject::Subgroup(receiver) => (receiver.group_id(), 0),
        IncomingObject::Datagram(object) => (object.location.group_id, object.location.object_id),
    }
}

impl SinkCutover {
    fn takes(&self, (group_id, object_id): (u64, u64)) -> bool {
        let mut shared = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.previous {
            return shared.cutover.take_previous(group_id);
        }
        let takes = shared.cutover.take_current(group_id, object_id);
        if shared.cutover.is_cut_over() {
            shared.cut_over = None;
        }
        takes
    }
}

impl SinkReceiver {
    fn deliver(&mut self, mut object: IncomingObject) -> IncomingObjectNotification {
        let location = location_of(&object);
        if self
            .cutover
            .as_ref()
            .is_some_and(|cutover| !cutover.takes(location))
        {
            return IncomingObjectNotification::Discarded;
        }
        self.newest_group = Some(
            self.newest_group
                .map_or(location.0, |newest| newest.max(location.0)),
        );
        if let IncomingObject::Subgroup(receiver) = &mut object {
            receiver.track_alias = self.logical_alias;
            receiver.count_in(&self.open_subgroups);
        }
        match self.sender.send(object) {
            Ok(()) => IncomingObjectNotification::Notified,
            Err(IncomingObjectSendError::Full) => IncomingObjectNotification::ReceiverFull,
            Err(IncomingObjectSendError::Closed) => IncomingObjectNotification::ReceiverClosed,
        }
    }
}

impl ObjectSink {
    fn is_receiver_registered(&self) -> bool {
        matches!(self, ObjectSink::Receiver(_))
    }

    fn created_at(&self) -> Option<Duration> {
        match self {
            ObjectSink::Buffer { created_at, .. } | ObjectSink::Cancelled { created_at } => {
                Some(*created_at)
            }
            ObjectSink::Receiver(_) => None,
        }
    }
}

#[derive(Default)]
pub(crate) struct ObjectSinks {
    sinks: HashMap<u64, ObjectSink>,
    /// The objects in every `ObjectSink::Buffer`.
    pending_objects: usize,
    /// The track aliases of the sinks that expire, oldest first, with their
    /// `created_at`. An alias whose sink has changed since is skipped.
    expiries: VecDeque<(Duration, u64)>,
}

impl ObjectSinks {
    fn notify(
        &mut self,
        track_alias: u64,
        incoming_object: IncomingObject,
        now: Duration,
    ) -> IncomingObjectNotification {
        self.remove_expired(now);
        match self.sinks.entry(track_alias) {
            Entry::Vacant(_) if self.pending_objects >= MAX_PENDING_OBJECTS => {
                IncomingObjectNotification::BufferFull
            }
            Entry::Vacant(entry) => {
                entry.insert(ObjectSink::Buffer {
                    objects: VecDeque::from([incoming_object]),
                    created_at: now,
                });
                self.pending_objects += 1;
                self.expiries.push_back((now, track_alias));
                IncomingObjectNotification::Buffered {
                    pending_objects: 1,
                    dropped_oldest: false,
                }
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                ObjectSink::Receiver(receiver) => receiver.deliver(incoming_object),
                ObjectSink::Buffer { objects, .. } => {
                    let dropped_oldest = objects.len() >= MAX_PENDING_OBJECTS_PER_TRACK_ALIAS;
                    if dropped_oldest {
                        objects.pop_front();
                    } else if self.pending_objects >= MAX_PENDING_OBJECTS {
                        return IncomingObjectNotification::BufferFull;
                    } else {
                        self.pending_objects += 1;
                    }
                    objects.push_back(incoming_object);
                    IncomingObjectNotification::Buffered {
                        pending_objects: objects.len(),
                        dropped_oldest,
                    }
                }
                ObjectSink::Cancelled { .. } => IncomingObjectNotification::Discarded,
            },
        }
    }

    fn install(
        &mut self,
        track_alias: u64,
        mut receiver: SinkReceiver,
    ) -> Result<(), TerminationErrorCode> {
        if self
            .sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            return Err(TerminationErrorCode::DuplicateTrackAlias);
        }
        if let Some(ObjectSink::Buffer { objects, .. }) = self.sinks.remove(&track_alias) {
            self.pending_objects -= objects.len();
            for incoming_object in objects {
                receiver.deliver(incoming_object);
            }
        }
        self.sinks
            .insert(track_alias, ObjectSink::Receiver(receiver));
        Ok(())
    }

    fn remove_expired(&mut self, now: Duration) {
        while let Some(&(created_at, track_alias)) = self.expiries.front()
            && created_at + UNKNOWN_TRACK_ALIAS_PERIOD <= now
        {
            self.expiries.pop_front();
            if let Entry::Occupied(entry) = self.sinks.entry(track_alias)
                && entry.get().created_at() == Some(created_at)
            {
                let removed = entry.remove();
                self.release(removed);
            }
        }
    }

    /// The sink of the subscription `request_id`, with its track alias.
    fn receiver_mut(&mut self, request_id: RequestId) -> Option<(u64, &mut SinkReceiver)> {
        self.sinks
            .iter_mut()
            .find_map(|(track_alias, sink)| match sink {
                ObjectSink::Receiver(receiver) if receiver.request_id == request_id => {
                    Some((*track_alias, receiver))
                }
                _ => None,
            })
    }

    /// Cancels the sink of the subscription `request_id` and returns its
    /// logical track alias. `None` when no such sink is installed.
    pub(crate) fn cancel_subscription(
        &mut self,
        request_id: RequestId,
        now: Duration,
    ) -> Option<u64> {
        let (track_alias, receiver) = self.receiver_mut(request_id)?;
        let logical_alias = receiver.logical_alias;
        self.cancel(track_alias, now);
        Some(logical_alias)
    }

    pub(crate) fn cancel_unclaimed(&mut self, track_alias: u64, now: Duration) {
        if !self
            .sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            self.cancel(track_alias, now);
        }
    }

    fn cancel(&mut self, track_alias: u64, now: Duration) {
        self.remove_expired(now);
        if let Some(replaced) = self
            .sinks
            .insert(track_alias, ObjectSink::Cancelled { created_at: now })
        {
            self.release(replaced);
        }
        self.expiries.push_back((now, track_alias));
    }

    fn release(&mut self, removed: ObjectSink) {
        if let ObjectSink::Buffer { objects, .. } = removed {
            self.pending_objects -= objects.len();
        }
    }
}

#[derive(Debug)]
pub(crate) enum IncomingObjectNotification {
    Notified,
    Buffered {
        pending_objects: usize,
        dropped_oldest: bool,
    },
    /// The connection buffers `MAX_PENDING_OBJECTS` already.
    BufferFull,
    /// `MAX_QUEUED_DATAGRAMS` datagrams queued for the receiver are unread.
    ReceiverFull,
    ReceiverClosed,
    Discarded,
}

impl Connection {
    pub(crate) fn new(
        transport: BoxedConnection,
        control: ControlChannel,
        is_server: bool,
        generation: u64,
        event_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        Self {
            transport,
            control,
            generation,
            event_sender,
            is_server,
            go_away_received: AtomicBool::new(false),
            closed_locally: AtomicBool::new(false),
            track_alias: AtomicU64::new(0),
            carried: std::sync::Mutex::default(),
            served: std::sync::Mutex::default(),
            object_sinks: tokio::sync::Mutex::default(),
            fetch_notification_map: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn logical_request_id(&self, wire_id: u64) -> u64 {
        self.carried_ids()
            .logical_by_wire
            .get(&wire_id)
            .copied()
            .unwrap_or_else(|| logical_id(self.generation, wire_id))
    }

    /// `None` when the request was never sent on this connection.
    pub(crate) fn wire_request_id(&self, logical_id: u64) -> Option<u64> {
        if generation_of(logical_id) == self.generation {
            return Some(wire_part(logical_id));
        }
        self.carried_ids().wire_by_logical.get(&logical_id).copied()
    }

    /// Records that the request `logical_id` was sent again here as `wire_id`.
    pub(crate) fn carry(&self, logical_id: u64, wire_id: u64) {
        let mut carried = self.carried_ids();
        carried.wire_by_logical.insert(logical_id, wire_id);
        carried.logical_by_wire.insert(wire_id, logical_id);
    }

    pub(crate) fn serve(&self, request_id: u64) {
        self.served_ids().insert(request_id);
    }

    pub(crate) fn stop_serving(&self, request_id: u64) {
        self.served_ids().remove(&request_id);
    }

    pub(crate) fn take_served(&self) -> Vec<u64> {
        self.served_ids().drain().collect()
    }

    pub(crate) fn serves_subscriptions(&self) -> bool {
        !self.served_ids().is_empty()
    }

    fn served_ids(&self) -> std::sync::MutexGuard<'_, HashSet<u64>> {
        self.served
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn carried_ids(&self) -> std::sync::MutexGuard<'_, CarriedIds> {
        self.carried
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) async fn notify_incoming_object(
        &self,
        track_alias: u64,
        incoming_object: IncomingObject,
    ) -> IncomingObjectNotification {
        self.object_sinks
            .lock()
            .await
            .notify(track_alias, incoming_object, executor::now())
    }

    /// Installs `receiver` for `track_alias` and hands it the objects that
    /// arrived before. Fails when another subscription holds the alias. The
    /// sinks stay locked while the returned guard lives.
    pub(crate) async fn install_receiver(
        &self,
        track_alias: u64,
        receiver: SinkReceiver,
    ) -> Result<tokio::sync::MutexGuard<'_, ObjectSinks>, TerminationErrorCode> {
        let mut sinks = self.object_sinks.lock().await;
        sinks.install(track_alias, receiver)?;
        Ok(sinks)
    }

    /// The newest group the sink of the subscription `wire_request_id`
    /// handed to the application, and its subgroup streams still open.
    /// `None` when no such sink is installed.
    pub(crate) async fn receiver_progress(
        &self,
        wire_request_id: RequestId,
    ) -> Option<(Option<u64>, OpenSubgroups)> {
        let mut sinks = self.object_sinks.lock().await;
        let (_, receiver) = sinks.receiver_mut(wire_request_id)?;
        Some((receiver.newest_group, receiver.open_subgroups.clone()))
    }

    /// Starts a cutover for the sink of the subscription `wire_request_id`,
    /// which becomes its previous side; the current side delivers whole
    /// groups from `current_start`. `None` when no such sink is installed.
    pub(crate) async fn start_cutover(
        &self,
        wire_request_id: RequestId,
        cut_over: oneshot::Sender<()>,
        current_start: u64,
    ) -> Option<CutoverStart> {
        let mut sinks = self.object_sinks.lock().await;
        let (_, receiver) = sinks.receiver_mut(wire_request_id)?;
        let shared = Arc::new(std::sync::Mutex::new(SharedCutover {
            cutover: GroupCutover::waiting_after(receiver.newest_group, current_start),
            cut_over: Some(cut_over),
        }));
        receiver.cutover = Some(SinkCutover {
            shared: shared.clone(),
            previous: true,
        });
        Some(CutoverStart {
            logical_alias: receiver.logical_alias,
            sender: receiver.sender.clone(),
            shared,
            previous_open_subgroups: receiver.open_subgroups.clone(),
        })
    }

    /// draft-14 §9.4: a second GOAWAY, or a New Session URI sent to a
    /// server, is a protocol violation.
    pub(crate) fn note_go_away_received(&self, new_session_uri: &str) -> Result<(), &'static str> {
        if self.go_away_received.swap(true, Ordering::SeqCst) {
            return Err("received a second GOAWAY");
        }
        if self.is_server && !new_session_uri.is_empty() {
            return Err("GOAWAY sent to a server carries a New Session URI");
        }
        Ok(())
    }

    /// A closed connection delivers no more FETCH streams: dropping their
    /// senders makes `accept_fetch_receiver` fail instead of waiting.
    pub(crate) fn end_pending_fetches(&self) {
        self.fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .clear();
    }

    pub(crate) fn allocate_track_alias(&self) -> u64 {
        self.track_alias.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn close(&self, code: TerminationErrorCode, reason: &str) {
        self.closed_locally.store(true, Ordering::SeqCst);
        self.transport
            .close(self.control.version().termination_code(code), reason);
    }

    /// Whether this side closed the connection, rather than the peer or the
    /// network.
    pub(crate) fn closed_locally(&self) -> bool {
        self.closed_locally.load(Ordering::SeqCst)
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        IncomingObjectNotification, MAX_PENDING_OBJECTS, MAX_PENDING_OBJECTS_PER_TRACK_ALIAS,
        ObjectSinks, SinkReceiver, UNKNOWN_TRACK_ALIAS_PERIOD,
    };
    use crate::modules::{
        moqt::{
            data_plane::stream::open_subgroups::OpenSubgroups,
            runtime::dispatch::incoming_object::{IncomingObject, incoming_object_channel},
        },
        test_support::datagram_object,
    };

    const TRACK_ALIAS: u64 = 5;
    const OTHER_TRACK_ALIAS: u64 = 7;
    const START: Duration = Duration::from_secs(1);

    fn notify_datagram(
        sinks: &mut ObjectSinks,
        track_alias: u64,
        now: Duration,
    ) -> IncomingObjectNotification {
        sinks.notify(
            track_alias,
            IncomingObject::Datagram(datagram_object()),
            now,
        )
    }

    /// Buffers `MAX_PENDING_OBJECTS` datagrams on aliases from 100 up.
    fn full_sinks() -> ObjectSinks {
        let mut sinks = ObjectSinks::default();
        for index in 0..MAX_PENDING_OBJECTS {
            let track_alias = 100 + (index / MAX_PENDING_OBJECTS_PER_TRACK_ALIAS) as u64;
            notify_datagram(&mut sinks, track_alias, START);
        }
        sinks
    }

    fn sink_receiver() -> SinkReceiver {
        SinkReceiver {
            request_id: 1,
            sender: incoming_object_channel().0,
            logical_alias: 100,
            newest_group: None,
            cutover: None,
            open_subgroups: OpenSubgroups::new(),
        }
    }

    #[test]
    fn a_buffer_is_kept_within_the_unknown_track_alias_period() {
        // Arrange
        let mut sinks = ObjectSinks::default();
        notify_datagram(&mut sinks, TRACK_ALIAS, START);

        // Act
        notify_datagram(
            &mut sinks,
            OTHER_TRACK_ALIAS,
            START + UNKNOWN_TRACK_ALIAS_PERIOD - Duration::from_millis(1),
        );

        // Assert
        assert!(sinks.sinks.contains_key(&TRACK_ALIAS));
        assert_eq!(sinks.pending_objects, 2);
    }

    #[test]
    fn a_buffer_is_dropped_once_the_unknown_track_alias_period_ends() {
        // Arrange
        let mut sinks = ObjectSinks::default();
        notify_datagram(&mut sinks, TRACK_ALIAS, START);
        notify_datagram(&mut sinks, TRACK_ALIAS, START);

        // Act
        notify_datagram(
            &mut sinks,
            OTHER_TRACK_ALIAS,
            START + UNKNOWN_TRACK_ALIAS_PERIOD,
        );

        // Assert
        assert!(!sinks.sinks.contains_key(&TRACK_ALIAS));
        assert_eq!(sinks.pending_objects, 1);
    }

    #[test]
    fn an_object_of_a_new_track_alias_is_dropped_once_the_connection_buffers_the_limit() {
        // Arrange
        let mut sinks = full_sinks();

        // Act
        let notification = notify_datagram(&mut sinks, TRACK_ALIAS, START);

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::BufferFull),
            "{notification:?}"
        );
        assert!(!sinks.sinks.contains_key(&TRACK_ALIAS));
    }

    #[test]
    fn an_object_of_a_buffered_track_alias_is_dropped_once_the_connection_buffers_the_limit() {
        // Arrange
        let mut sinks = full_sinks();
        sinks.install(100, sink_receiver()).unwrap();
        notify_datagram(&mut sinks, TRACK_ALIAS, START);
        for _ in 0..MAX_PENDING_OBJECTS_PER_TRACK_ALIAS - 1 {
            notify_datagram(&mut sinks, OTHER_TRACK_ALIAS, START);
        }

        // Act
        let notification = notify_datagram(&mut sinks, TRACK_ALIAS, START);

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::BufferFull),
            "{notification:?}"
        );
        assert_eq!(sinks.pending_objects, MAX_PENDING_OBJECTS);
    }

    #[test]
    fn installing_a_receiver_makes_room_for_other_track_aliases() {
        // Arrange
        let mut sinks = full_sinks();

        // Act
        sinks.install(100, sink_receiver()).unwrap();
        let notification = notify_datagram(&mut sinks, TRACK_ALIAS, START);

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::Buffered { .. }),
            "{notification:?}"
        );
    }

    #[test]
    fn cancelling_a_buffered_track_alias_makes_room_for_other_track_aliases() {
        // Arrange
        let mut sinks = full_sinks();

        // Act
        sinks.cancel_unclaimed(100, START);
        let notification = notify_datagram(&mut sinks, TRACK_ALIAS, START);

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::Buffered { .. }),
            "{notification:?}"
        );
    }

    #[test]
    fn a_cancelled_track_alias_discards_its_objects_within_the_period() {
        // Arrange
        let mut sinks = ObjectSinks::default();
        sinks.cancel_unclaimed(TRACK_ALIAS, START);

        // Act
        let notification = notify_datagram(
            &mut sinks,
            TRACK_ALIAS,
            START + UNKNOWN_TRACK_ALIAS_PERIOD - Duration::from_millis(1),
        );

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::Discarded),
            "{notification:?}"
        );
    }

    #[test]
    fn a_cancelled_track_alias_is_forgotten_once_the_period_ends() {
        // Arrange
        let mut sinks = ObjectSinks::default();
        sinks.install(TRACK_ALIAS, sink_receiver()).unwrap();
        sinks.cancel_subscription(1, START);

        // Act: the next cancellation drops it, with no object arriving
        sinks.cancel_unclaimed(OTHER_TRACK_ALIAS, START + UNKNOWN_TRACK_ALIAS_PERIOD);

        // Assert
        assert!(!sinks.sinks.contains_key(&TRACK_ALIAS));
        assert_eq!(sinks.expiries.len(), 1);
    }
}

use std::{
    collections::{HashMap, HashSet, VecDeque, hash_map::Entry},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use tokio::sync::{mpsc, oneshot};

use crate::{
    SessionEvent,
    modules::{
        moqt::{
            control_plane::{constants::TerminationErrorCode, enums::RequestId},
            data_plane::stream::fetch_data_receiver::FetchDataReceiver,
            domains::group_cutover::GroupCutover,
            runtime::dispatch::incoming_object::IncomingObject,
            version::ControlChannel,
        },
        transport::transport_connection::BoxedConnection,
    },
};

const MAX_PENDING_OBJECTS_PER_TRACK_ALIAS: usize = 256;
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
    track_alias: AtomicU64,
    /// Requests of earlier connections sent again here: logical id ↔ wire id.
    carried: std::sync::Mutex<CarriedIds>,
    /// The logical ids of the SUBSCRIBEs the peer sent here that this side
    /// accepted and neither side has ended; kept for a session that follows
    /// GOAWAY, which closes a previous connection only once it is empty.
    served: std::sync::Mutex<HashSet<u64>>,
    pub(crate) object_sinks: tokio::sync::Mutex<HashMap<u64, ObjectSink>>,
    pub(crate) fetch_notification_map:
        std::sync::Mutex<HashMap<RequestId, mpsc::UnboundedSender<FetchDataReceiver>>>,
}

#[derive(Default)]
struct CarriedIds {
    wire_by_logical: HashMap<u64, u64>,
    logical_by_wire: HashMap<u64, u64>,
}

pub(crate) enum ObjectSink {
    /// Buffers objects that arrive before the receiver is registered.
    Buffer(VecDeque<IncomingObject>),
    Receiver(SinkReceiver),
    /// Draft-14 §10.1: objects of a cancelled subscription can still arrive
    /// and are discarded instead of being buffered as an unknown track alias.
    /// Kept until a new subscription reuses the track alias.
    Cancelled,
}

pub(crate) struct SinkReceiver {
    /// The subscription's wire request id on this connection.
    pub(crate) request_id: RequestId,
    pub(crate) sender: mpsc::UnboundedSender<IncomingObject>,
    /// The track alias the application knows; set on every stream it gets.
    pub(crate) logical_alias: u64,
    /// The newest group handed to the application, where a cutover starts.
    pub(crate) newest_group: Option<u64>,
    /// Set while the subscription is read on two connections.
    pub(crate) cutover: Option<SinkCutover>,
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
        }
        if self.sender.send(object).is_err() {
            IncomingObjectNotification::ReceiverClosed
        } else {
            IncomingObjectNotification::Notified
        }
    }
}

impl ObjectSink {
    pub(crate) fn is_receiver_registered(&self) -> bool {
        matches!(self, ObjectSink::Receiver(_))
    }
}

#[derive(Debug)]
pub(crate) enum IncomingObjectNotification {
    Notified,
    Buffered {
        pending_objects: usize,
        dropped_oldest: bool,
    },
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
            track_alias: AtomicU64::new(0),
            carried: std::sync::Mutex::default(),
            served: std::sync::Mutex::default(),
            object_sinks: tokio::sync::Mutex::new(HashMap::new()),
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

    /// `true` when the subscription was served here.
    pub(crate) fn stop_serving(&self, request_id: u64) -> bool {
        self.served_ids().remove(&request_id)
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
        let mut sinks = self.object_sinks.lock().await;
        match sinks.entry(track_alias) {
            Entry::Vacant(entry) => {
                let mut objects = VecDeque::new();
                objects.push_back(incoming_object);
                entry.insert(ObjectSink::Buffer(objects));
                IncomingObjectNotification::Buffered {
                    pending_objects: 1,
                    dropped_oldest: false,
                }
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                ObjectSink::Receiver(receiver) => receiver.deliver(incoming_object),
                ObjectSink::Buffer(objects) => {
                    let dropped_oldest = objects.len() >= MAX_PENDING_OBJECTS_PER_TRACK_ALIAS;
                    if dropped_oldest {
                        objects.pop_front();
                    }
                    objects.push_back(incoming_object);
                    IncomingObjectNotification::Buffered {
                        pending_objects: objects.len(),
                        dropped_oldest,
                    }
                }
                ObjectSink::Cancelled => IncomingObjectNotification::Discarded,
            },
        }
    }

    /// Installs `receiver` for `track_alias` and hands it the objects that
    /// arrived before. Fails when another subscription holds the alias. The
    /// sinks stay locked while the returned guard lives.
    pub(crate) async fn install_receiver(
        &self,
        track_alias: u64,
        mut receiver: SinkReceiver,
    ) -> Result<tokio::sync::MutexGuard<'_, HashMap<u64, ObjectSink>>, TerminationErrorCode> {
        let mut sinks = self.object_sinks.lock().await;
        if sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            return Err(TerminationErrorCode::DuplicateTrackAlias);
        }
        if let Some(ObjectSink::Buffer(pending_objects)) = sinks.remove(&track_alias) {
            for incoming_object in pending_objects {
                receiver.deliver(incoming_object);
            }
        }
        sinks.insert(track_alias, ObjectSink::Receiver(receiver));
        Ok(sinks)
    }

    /// Starts a cutover for the sink of the subscription `wire_request_id`,
    /// which becomes its previous side. Returns what the sink on the new
    /// connection needs: the application's track alias, the sender and the
    /// shared cutover. `None` when no such sink is installed.
    pub(crate) async fn start_cutover(
        &self,
        wire_request_id: RequestId,
        cut_over: oneshot::Sender<()>,
    ) -> Option<(
        u64,
        mpsc::UnboundedSender<IncomingObject>,
        Arc<std::sync::Mutex<SharedCutover>>,
    )> {
        let mut sinks = self.object_sinks.lock().await;
        let receiver = sinks.values_mut().find_map(|sink| match sink {
            ObjectSink::Receiver(receiver) if receiver.request_id == wire_request_id => {
                Some(receiver)
            }
            _ => None,
        })?;
        let shared = Arc::new(std::sync::Mutex::new(SharedCutover {
            cutover: GroupCutover::waiting_after(receiver.newest_group),
            cut_over: Some(cut_over),
        }));
        receiver.cutover = Some(SinkCutover {
            shared: shared.clone(),
            previous: true,
        });
        Some((receiver.logical_alias, receiver.sender.clone(), shared))
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

    pub(crate) fn allocate_track_alias(&self) -> u64 {
        self.track_alias.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn close(&self, code: TerminationErrorCode, reason: &str) {
        self.transport
            .close(self.control.version().termination_code(code), reason);
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

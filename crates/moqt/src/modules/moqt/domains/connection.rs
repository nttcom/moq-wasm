use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::modules::{
    moqt::{
        control_plane::{constants::TerminationErrorCode, enums::RequestId},
        data_plane::stream::fetch_data_receiver::FetchDataReceiver,
        runtime::dispatch::incoming_object::IncomingObject,
        version::ControlChannel,
    },
    transport::transport_connection::BoxedConnection,
};

const MAX_PENDING_OBJECTS_PER_TRACK_ALIAS: usize = 256;

/// One transport connection of a session and the state draft-14 scopes to
/// it: the control stream and its request ids (§9.1), the track aliases this
/// side assigns, whether the peer sent GOAWAY (§9.4), the objects arriving
/// per track alias and the FETCH streams arriving per request id.
pub(crate) struct Connection {
    pub(crate) transport: BoxedConnection,
    pub(crate) control: ControlChannel,
    is_server: bool,
    go_away_received: AtomicBool,
    track_alias: AtomicU64,
    pub(crate) object_sinks: tokio::sync::Mutex<HashMap<u64, ObjectSink>>,
    pub(crate) fetch_notification_map:
        std::sync::Mutex<HashMap<RequestId, tokio::sync::mpsc::UnboundedSender<FetchDataReceiver>>>,
}

pub(crate) enum ObjectSink {
    /// Buffers objects that arrive before the receiver is registered.
    Buffer(VecDeque<IncomingObject>),
    Receiver {
        request_id: RequestId,
        sender: tokio::sync::mpsc::UnboundedSender<IncomingObject>,
    },
    /// Draft-14 §10.1: objects of a cancelled subscription can still arrive
    /// and are discarded instead of being buffered as an unknown track alias.
    /// Kept until a new subscription reuses the track alias.
    Cancelled,
}

impl ObjectSink {
    pub(crate) fn is_receiver_registered(&self) -> bool {
        matches!(self, ObjectSink::Receiver { .. })
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
    ) -> Self {
        Self {
            transport,
            control,
            is_server,
            go_away_received: AtomicBool::new(false),
            track_alias: AtomicU64::new(0),
            object_sinks: tokio::sync::Mutex::new(HashMap::new()),
            fetch_notification_map: std::sync::Mutex::new(HashMap::new()),
        }
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
                ObjectSink::Receiver { sender, .. } => {
                    if sender.send(incoming_object).is_err() {
                        IncomingObjectNotification::ReceiverClosed
                    } else {
                        IncomingObjectNotification::Notified
                    }
                }
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
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}

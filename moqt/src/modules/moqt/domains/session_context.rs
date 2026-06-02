use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    SessionEvent, TransportProtocol,
    modules::moqt::{
        control_plane::enums::{RequestId, ResponseMessage},
        data_plane::stream::bi_stream_sender::BiStreamSender,
        runtime::dispatch::incoming_object::IncomingObject,
    },
};

pub(crate) struct SessionContext<T: TransportProtocol> {
    pub(crate) transport_connection: T::Connection,
    pub(crate) send_stream: BiStreamSender<T>,
    request_id: AtomicU64,
    track_alias: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    pub(crate) sender_map:
        tokio::sync::Mutex<HashMap<RequestId, tokio::sync::oneshot::Sender<ResponseMessage>>>,
    pub(crate) receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject<T>>>>,
    object_sinks: tokio::sync::Mutex<HashMap<u64, ObjectSink<T>>>,
    pub(crate) fetch_notification_map:
        tokio::sync::RwLock<HashMap<u64, tokio::sync::mpsc::UnboundedSender<IncomingObject<T>>>>,
    pub(crate) fetch_receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject<T>>>>,
}

enum ObjectSink<T: TransportProtocol> {
    Buffering(VecDeque<IncomingObject<T>>),
    Live(tokio::sync::mpsc::UnboundedSender<IncomingObject<T>>),
}

pub(crate) enum IncomingObjectNotification {
    Notified,
    Buffered {
        pending_objects: usize,
        dropped_oldest: bool,
    },
    ReceiverClosed,
}

/// Outcome of attaching a data receiver to a track, used for logging by callers.
pub(crate) struct DataReceiverRegistration {
    pub(crate) already_registered: bool,
    pub(crate) drained_pending: usize,
    pub(crate) failed_to_drain: bool,
    pub(crate) replaced_receiver: bool,
}

impl<T: TransportProtocol> SessionContext<T> {
    pub(crate) fn new(
        transport_connection: T::Connection,
        send_stream: BiStreamSender<T>,
        request_id: AtomicU64,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    ) -> Self {
        Self {
            transport_connection,
            send_stream,
            request_id,
            track_alias: AtomicU64::new(0),
            event_sender,
            sender_map: tokio::sync::Mutex::new(HashMap::new()),
            receiver_map: tokio::sync::Mutex::new(HashMap::new()),
            object_sinks: tokio::sync::Mutex::new(HashMap::new()),
            fetch_notification_map: tokio::sync::RwLock::new(HashMap::new()),
            fetch_receiver_map: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn get_request_id(&self) -> u64 {
        let id = self.request_id.load(Ordering::SeqCst);
        tracing::debug!("request_id: {}", id);
        self.request_id.fetch_add(2, Ordering::SeqCst);
        id
    }

    pub(crate) fn get_track_alias(&self) -> u64 {
        let track_alias = self.track_alias.fetch_add(1, Ordering::SeqCst);
        tracing::debug!("track_alias: {}", track_alias);
        track_alias
    }

    pub(crate) async fn notify_incoming_object(
        &self,
        track_alias: u64,
        incoming_object: IncomingObject<T>,
        max_pending_objects: usize,
    ) -> IncomingObjectNotification {
        let mut sinks = self.object_sinks.lock().await;
        match sinks.entry(track_alias) {
            Entry::Vacant(entry) => {
                let mut objects = VecDeque::new();
                objects.push_back(incoming_object);
                entry.insert(ObjectSink::Buffering(objects));
                IncomingObjectNotification::Buffered {
                    pending_objects: 1,
                    dropped_oldest: false,
                }
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                ObjectSink::Live(sender) => {
                    if sender.send(incoming_object).is_err() {
                        IncomingObjectNotification::ReceiverClosed
                    } else {
                        IncomingObjectNotification::Notified
                    }
                }
                ObjectSink::Buffering(objects) => {
                    let dropped_oldest = objects.len() >= max_pending_objects;
                    if dropped_oldest {
                        objects.pop_front();
                    }
                    objects.push_back(incoming_object);
                    IncomingObjectNotification::Buffered {
                        pending_objects: objects.len(),
                        dropped_oldest,
                    }
                }
            },
        }
    }

    /// Attaches a data receiver to `track_alias` and returns it ready to consume.
    ///
    /// This owns the whole hand-off in one place: it creates the channel, flushes
    /// any objects that arrived before the receiver was registered (buffered by
    /// [`Self::notify_incoming_object`] — data objects can reach a relay before the
    /// matching PUBLISH/SUBSCRIBE_OK), promotes the sink to `Live`, and stores the
    /// receiving end in `receiver_map` so it can be picked up later. Both maps are
    /// updated under the object-sinks lock so the registration is atomic. If a
    /// receiver is already attached the existing one is left untouched and
    /// `already_registered` is returned.
    pub(crate) async fn register_data_receiver(
        &self,
        track_alias: u64,
    ) -> DataReceiverRegistration {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<IncomingObject<T>>();
        let mut sinks = self.object_sinks.lock().await;

        if let Some(ObjectSink::Live(_)) = sinks.get(&track_alias) {
            return DataReceiverRegistration {
                already_registered: true,
                drained_pending: 0,
                failed_to_drain: false,
                replaced_receiver: false,
            };
        }

        // Flush objects that arrived before this receiver existed, then go Live.
        let (drained_pending, failed_to_drain) = match sinks.remove(&track_alias) {
            Some(ObjectSink::Buffering(mut pending_objects)) => {
                let count = pending_objects.len();
                let mut failed_to_drain = false;
                while let Some(incoming_object) = pending_objects.pop_front() {
                    if sender.send(incoming_object).is_err() {
                        failed_to_drain = true;
                        break;
                    }
                }
                (count, failed_to_drain)
            }
            _ => (0, false),
        };
        sinks.insert(track_alias, ObjectSink::Live(sender));

        let replaced_receiver = self
            .receiver_map
            .lock()
            .await
            .insert(track_alias, receiver)
            .is_some();

        DataReceiverRegistration {
            already_registered: false,
            drained_pending,
            failed_to_drain,
            replaced_receiver,
        }
    }
}

impl<T: TransportProtocol> fmt::Debug for SessionContext<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionContext").finish_non_exhaustive()
    }
}

impl<T: TransportProtocol> Drop for SessionContext<T> {
    fn drop(&mut self) {
        tracing::info!("SessionContext dropped.");
        // send goaway
    }
}

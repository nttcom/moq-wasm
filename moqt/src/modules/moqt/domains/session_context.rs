use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use crate::{
    SessionEvent, TransportProtocol,
    modules::{
        moqt::{
            control_plane::{
                constants::TerminationErrorCode,
                enums::{RequestId, ResponseMessage},
            },
            data_plane::stream::bi_stream_sender::BiStreamSender,
            runtime::dispatch::incoming_object::IncomingObject,
        },
        transport::transport_connection::TransportConnection,
    },
};

pub(crate) struct SessionContext<T: TransportProtocol> {
    pub(crate) transport_connection: T::Connection,
    pub(crate) send_stream: BiStreamSender<T>,
    request_id: AtomicU64,
    track_alias: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    pub(crate) sender_map:
        std::sync::Mutex<HashMap<RequestId, tokio::sync::oneshot::Sender<ResponseMessage>>>,
    pub(crate) receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject<T>>>>,
    object_sinks: tokio::sync::Mutex<HashMap<u64, ObjectSink<T>>>,
    pub(crate) fetch_notification_map:
        tokio::sync::RwLock<HashMap<u64, tokio::sync::mpsc::UnboundedSender<IncomingObject<T>>>>,
    pub(crate) fetch_receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject<T>>>>,
}

enum ObjectSink<T: TransportProtocol> {
    /// Buffers objects that arrive before the receiver is registered.
    Buffer(VecDeque<IncomingObject<T>>),
    Receiver(tokio::sync::mpsc::UnboundedSender<IncomingObject<T>>),
}

impl<T: TransportProtocol> ObjectSink<T> {
    fn is_receiver_registered(&self) -> bool {
        matches!(self, ObjectSink::Receiver(_))
    }
}

pub(crate) enum IncomingObjectNotification {
    Notified,
    Buffered {
        pending_objects: usize,
        dropped_oldest: bool,
    },
    ReceiverClosed,
}

/// Holds the `sender_map` registration for an in-flight request. Removes the
/// entry on drop, so a cancelled request does not leave an orphaned sender behind.
#[must_use = "dropping this removes the sender from sender_map; bind it for the lifetime of the request"]
pub(crate) struct RegisteredSender<T: TransportProtocol> {
    session: Arc<SessionContext<T>>,
    request_id: RequestId,
}

impl<T: TransportProtocol> Drop for RegisteredSender<T> {
    fn drop(&mut self) {
        if let Ok(mut senders) = self.session.sender_map.lock() {
            senders.remove(&self.request_id);
        }
    }
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
            sender_map: std::sync::Mutex::new(HashMap::new()),
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

    /// Inserts the sender into `sender_map` and returns a `RegisteredSender` that
    /// removes it on drop, ensuring cleanup on both normal completion and cancellation.
    pub(crate) fn register_response_sender(
        self: &Arc<Self>,
        request_id: RequestId,
        sender: tokio::sync::oneshot::Sender<ResponseMessage>,
    ) -> RegisteredSender<T> {
        self.sender_map
            .lock()
            .expect("sender_map poisoned")
            .insert(request_id, sender);
        RegisteredSender {
            session: self.clone(),
            request_id,
        }
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
                entry.insert(ObjectSink::Buffer(objects));
                IncomingObjectNotification::Buffered {
                    pending_objects: 1,
                    dropped_oldest: false,
                }
            }
            Entry::Occupied(mut entry) => match entry.get_mut() {
                ObjectSink::Receiver(sender) => {
                    if sender.send(incoming_object).is_err() {
                        IncomingObjectNotification::ReceiverClosed
                    } else {
                        IncomingObjectNotification::Notified
                    }
                }
                ObjectSink::Buffer(objects) => {
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

    /// Drains objects buffered before SUBSCRIBE_OK into a new receiver and goes Live,
    /// updating both maps under the object-sinks lock so the registration is atomic.
    /// Returns `Err(TerminationErrorCode::DuplicateTrackAlias)` if a receiver already exists.
    pub(crate) async fn register_data_receiver(
        &self,
        track_alias: u64,
    ) -> Result<(), TerminationErrorCode> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<IncomingObject<T>>();
        let mut sinks = self.object_sinks.lock().await;

        if sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            return Err(TerminationErrorCode::DuplicateTrackAlias);
        }

        if let Some(ObjectSink::Buffer(mut pending_objects)) = sinks.remove(&track_alias) {
            while let Some(incoming_object) = pending_objects.pop_front() {
                let _ = sender.send(incoming_object);
            }
        }
        sinks.insert(track_alias, ObjectSink::Receiver(sender));

        self.receiver_map.lock().await.insert(track_alias, receiver);

        Ok(())
    }

    pub(crate) fn close_with_error(&self, code: TerminationErrorCode, reason: &str) {
        if let Err(error) = self.event_sender.send(SessionEvent::ProtocolViolation()) {
            tracing::error!(?error, "failed to send protocol violation event");
        }
        self.transport_connection.close(code as u32, reason);
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

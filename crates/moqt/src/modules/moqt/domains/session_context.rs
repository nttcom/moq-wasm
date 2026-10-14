use crate::modules::transport::transport_connection::BoxedConnection;
use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use crate::{
    RequestFailure, SessionEvent,
    modules::moqt::{
        control_plane::{constants::TerminationErrorCode, enums::RequestId},
        data_plane::stream::fetch_data_receiver::FetchDataReceiver,
        runtime::dispatch::incoming_object::IncomingObject,
        version::{ControlChannel, OutgoingRequest, PendingResponse},
    },
};

const MAX_PENDING_OBJECTS_PER_TRACK_ALIAS: usize = 256;

pub(crate) struct SessionContext {
    pub(crate) transport_connection: BoxedConnection,
    pub(crate) control: ControlChannel,
    is_server: bool,
    go_away_received: AtomicBool,
    track_alias: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    pub(crate) receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject>>>,
    object_sinks: tokio::sync::Mutex<HashMap<u64, ObjectSink>>,
    pub(crate) fetch_notification_map:
        std::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedSender<FetchDataReceiver>>>,
    pub(crate) fetch_receiver_map:
        std::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<FetchDataReceiver>>>,
}

enum ObjectSink {
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
    fn is_receiver_registered(&self) -> bool {
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

#[must_use = "dropping this removes the pending fetch; bind it for the lifetime of the request"]
pub(crate) struct PendingFetch {
    session: Arc<SessionContext>,
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
            self.session.remove_pending_fetch(self.request_id);
        }
    }
}

impl SessionContext {
    pub(crate) fn new(
        transport_connection: BoxedConnection,
        control: ControlChannel,
        is_server: bool,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        Self {
            transport_connection,
            control,
            is_server,
            go_away_received: AtomicBool::new(false),
            track_alias: AtomicU64::new(0),
            event_sender,
            receiver_map: tokio::sync::Mutex::new(HashMap::new()),
            object_sinks: tokio::sync::Mutex::new(HashMap::new()),
            fetch_notification_map: std::sync::Mutex::new(HashMap::new()),
            fetch_receiver_map: std::sync::Mutex::new(HashMap::new()),
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

    pub(crate) fn get_track_alias(&self) -> u64 {
        self.track_alias.fetch_add(1, Ordering::SeqCst)
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

    /// `receiver_map` is updated while the object-sinks lock is held, so the
    /// drain, the sink switch and the receiver registration are atomic.
    pub(crate) async fn register_data_receiver(
        &self,
        request_id: RequestId,
        track_alias: u64,
    ) -> Result<(), TerminationErrorCode> {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<IncomingObject>();
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
        sinks.insert(track_alias, ObjectSink::Receiver { request_id, sender });

        self.receiver_map.lock().await.insert(track_alias, receiver);

        Ok(())
    }

    pub(crate) async fn cancel_subscription(&self, request_id: RequestId) {
        let mut sinks = self.object_sinks.lock().await;
        let track_alias = sinks.iter().find_map(|(track_alias, sink)| {
            matches!(sink, ObjectSink::Receiver { request_id: id, .. } if *id == request_id)
                .then_some(*track_alias)
        });
        if let Some(track_alias) = track_alias {
            self.cancel_track_alias(&mut sinks, track_alias).await;
        }
    }

    pub(crate) async fn cancel_unclaimed_track_alias(&self, track_alias: u64) {
        let mut sinks = self.object_sinks.lock().await;
        if !sinks
            .get(&track_alias)
            .is_some_and(ObjectSink::is_receiver_registered)
        {
            self.cancel_track_alias(&mut sinks, track_alias).await;
        }
    }

    async fn cancel_track_alias(&self, sinks: &mut HashMap<u64, ObjectSink>, track_alias: u64) {
        sinks.insert(track_alias, ObjectSink::Cancelled);
        self.receiver_map.lock().await.remove(&track_alias);
    }

    /// Call before sending the FETCH: its data stream can arrive before FETCH_OK.
    pub(crate) fn register_pending_fetch(self: &Arc<Self>, request_id: RequestId) -> PendingFetch {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<FetchDataReceiver>();
        self.fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .insert(request_id, sender);
        self.fetch_receiver_map
            .lock()
            .expect("fetch_receiver_map poisoned")
            .insert(request_id, receiver);
        PendingFetch {
            session: self.clone(),
            request_id,
            kept: false,
        }
    }

    pub(crate) fn remove_pending_fetch(&self, request_id: RequestId) {
        self.fetch_notification_map
            .lock()
            .expect("fetch_notification_map poisoned")
            .remove(&request_id);
        self.fetch_receiver_map
            .lock()
            .expect("fetch_receiver_map poisoned")
            .remove(&request_id);
    }

    pub(crate) async fn send_request(
        self: &Arc<Self>,
        request: OutgoingRequest,
    ) -> Result<PendingResponse, RequestFailure> {
        self.control.send_request(self, request).await
    }

    /// A close for a protocol violation is reported at once as
    /// `SessionEvent::ProtocolViolation`; any other close is reported as
    /// `SessionEvent::Disconnected` once the transport has closed, like a
    /// close by the peer.
    pub(crate) fn close_with_error(&self, code: TerminationErrorCode, reason: &str) {
        if code == TerminationErrorCode::ProtocolViolation
            && let Err(error) = self.event_sender.send(SessionEvent::ProtocolViolation())
        {
            tracing::error!(?error, "failed to send protocol violation event");
        }
        self.transport_connection
            .close(self.control.version().termination_code(code), reason);
    }
}

impl fmt::Debug for SessionContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionContext").finish_non_exhaustive()
    }
}

impl Drop for SessionContext {
    fn drop(&mut self) {
        tracing::debug!("SessionContext dropped.");
        // send goaway
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::IncomingObjectNotification;
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
                            .map(|_| context.control.next_request_id())
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
            .register_data_receiver(REQUEST_ID, TRACK_ALIAS)
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
            .register_data_receiver(REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();
        context.cancel_subscription(REQUEST_ID).await;

        // Act
        let registration = context
            .register_data_receiver(OTHER_REQUEST_ID, TRACK_ALIAS)
            .await;

        // Assert
        assert!(registration.is_ok());
    }
}

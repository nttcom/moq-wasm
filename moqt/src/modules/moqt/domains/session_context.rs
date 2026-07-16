use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::{
    SessionEvent, TransportProtocol,
    modules::{
        moqt::{
            control_plane::{
                constants::TerminationErrorCode,
                control_messages::{
                    control_message_type::ControlMessageType,
                    messages::{
                        publish_namespace_done::PublishNamespaceDone, unsubscribe::Unsubscribe,
                        unsubscribe_namespace::UnsubscribeNamespace,
                    },
                },
                enums::{RequestId, ResponseMessage},
            },
            data_plane::stream::bi_stream_sender::BiStreamSender,
            runtime::dispatch::incoming_object::IncomingObject,
        },
        transport::transport_connection::TransportConnection,
    },
};

const CONTROL_MESSAGE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Returned by [`SessionContext::await_response`] when the peer does
/// not answer in time. Callers map this to a per-request failure (e.g.
/// FETCH_ERROR TIMEOUT) and keep the session open.
#[derive(Debug)]
pub struct RequestTimeoutError;

impl fmt::Display for RequestTimeoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request response timed out")
    }
}

impl std::error::Error for RequestTimeoutError {}

/// How to withdraw a request whose success response arrives after the caller
/// stopped waiting (timeout or cancellation). Late error responses never need
/// a withdrawal and are always discarded.
#[derive(Debug)]
pub(crate) enum LateResponseAction {
    /// Discard the late response without a withdrawal message.
    ///
    /// FIXME: PUBLISH and FETCH should withdraw with PUBLISH_DONE /
    /// FETCH_CANCEL, but decoding those messages is still `todo!()`, so
    /// sending them would crash a peer running this crate.
    Discard,
    /// A late SUBSCRIBE_OK established a subscription nobody consumes.
    Unsubscribe,
    /// A late SUBSCRIBE_NAMESPACE_OK registered an unwanted namespace interest.
    UnsubscribeNamespace { namespace: Vec<String> },
    /// A late PUBLISH_NAMESPACE_OK accepted an announcement we gave up on.
    PublishNamespaceDone { namespace: Vec<String> },
}

/// A request awaiting its response on this session. Kept in `sender_map`
/// until the response arrives, even after the caller stops waiting, so a
/// late response is answered with a withdrawal instead of being mistaken
/// for an unknown Request ID (which must close the session, §9.1).
pub(crate) enum InflightRequest {
    Waiting {
        sender: tokio::sync::oneshot::Sender<ResponseMessage>,
        on_late_response: LateResponseAction,
    },
    Abandoned(LateResponseAction),
}

pub(crate) struct SessionContext<T: TransportProtocol> {
    pub(crate) transport_connection: T::Connection,
    pub(crate) send_stream: BiStreamSender<T>,
    request_id: AtomicU64,
    track_alias: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent<T>>,
    pub(crate) sender_map: std::sync::Mutex<HashMap<RequestId, InflightRequest>>,
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

/// Holds the `sender_map` registration for an in-flight request. On drop, a
/// still-waiting entry is downgraded to `Abandoned` (not removed): the caller
/// gave up, but a late response must still find the entry so it triggers the
/// registered action instead of a session close.
#[must_use = "dropping this abandons the request in sender_map; bind it for the lifetime of the request"]
pub(crate) struct RegisteredSender<T: TransportProtocol> {
    session: Arc<SessionContext<T>>,
    request_id: RequestId,
}

impl<T: TransportProtocol> Drop for RegisteredSender<T> {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.session.sender_map.lock()
            && let Some(entry) = requests.get_mut(&self.request_id)
            && let InflightRequest::Waiting {
                on_late_response, ..
            } = entry
        {
            let action = std::mem::replace(on_late_response, LateResponseAction::Discard);
            *entry = InflightRequest::Abandoned(action);
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

    /// Inserts the sender into `sender_map` and returns a `RegisteredSender`
    /// that marks the request abandoned on drop. `on_late_response` is the
    /// withdrawal to send if a success response arrives after abandonment.
    pub(crate) fn register_response_sender(
        self: &Arc<Self>,
        request_id: RequestId,
        sender: tokio::sync::oneshot::Sender<ResponseMessage>,
        on_late_response: LateResponseAction,
    ) -> RegisteredSender<T> {
        self.sender_map.lock().expect("sender_map poisoned").insert(
            request_id,
            InflightRequest::Waiting {
                sender,
                on_late_response,
            },
        );
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

    /// Awaits a response to a control message with a bounded timeout (§12.2).
    ///
    /// A timeout fails only this request instead of closing the session:
    /// requests are request-scoped, and a shared (inter-relay) session must
    /// survive one slow request. §12.2's resource-exhaustion concern is met
    /// by the caller dropping its pending request state on the error path;
    /// the `sender_map` entry stays behind as `Abandoned` so a late response
    /// is withdrawn instead of closing the session.
    pub(crate) async fn await_response(
        &self,
        receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    ) -> anyhow::Result<ResponseMessage> {
        match tokio::time::timeout(CONTROL_MESSAGE_RESPONSE_TIMEOUT, receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => anyhow::bail!("control response channel closed: {}", error),
            Err(_) => Err(RequestTimeoutError.into()),
        }
    }

    /// Handles a response that arrived after the caller abandoned the request.
    /// A success response means the peer holds state (subscription, namespace
    /// interest, ...) nobody on this side will consume, so send the matching
    /// withdrawal; error responses are simply discarded.
    pub(crate) async fn handle_late_response(
        &self,
        request_id: RequestId,
        action: LateResponseAction,
        response: ResponseMessage,
    ) {
        // The action fires only for the success response it was registered
        // for: an error response leaves no peer state to withdraw, and a
        // mismatched response type must not trigger a blind withdrawal.
        let send_result = match (&action, &response) {
            (LateResponseAction::Unsubscribe, ResponseMessage::SubscribeOk(_)) => {
                tracing::warn!(
                    request_id,
                    "SUBSCRIBE_OK arrived after the request was abandoned; sending UNSUBSCRIBE"
                );
                self.send_stream
                    .send(
                        ControlMessageType::UnSubscribe,
                        Unsubscribe { request_id }.encode(),
                    )
                    .await
            }
            (
                LateResponseAction::UnsubscribeNamespace { namespace },
                ResponseMessage::SubscribeNameSpaceOk(_),
            ) => {
                tracing::warn!(
                    request_id,
                    "SUBSCRIBE_NAMESPACE_OK arrived after the request was abandoned; sending UNSUBSCRIBE_NAMESPACE"
                );
                self.send_stream
                    .send(
                        ControlMessageType::UnSubscribeNamespace,
                        UnsubscribeNamespace::new(namespace.clone()).encode(),
                    )
                    .await
            }
            (
                LateResponseAction::PublishNamespaceDone { namespace },
                ResponseMessage::PublishNamespaceOk(_),
            ) => {
                tracing::warn!(
                    request_id,
                    "PUBLISH_NAMESPACE_OK arrived after the request was abandoned; sending PUBLISH_NAMESPACE_DONE"
                );
                self.send_stream
                    .send(
                        ControlMessageType::PublishNamespaceDone,
                        PublishNamespaceDone::new(namespace.clone()).encode(),
                    )
                    .await
            }
            _ => {
                tracing::warn!(
                    request_id,
                    ?response,
                    "discarding response that arrived after the request was abandoned"
                );
                Ok(())
            }
        };
        if let Err(error) = send_result {
            tracing::warn!(
                request_id,
                %error,
                "failed to withdraw an abandoned request"
            );
        }
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

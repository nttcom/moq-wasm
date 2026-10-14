use crate::modules::transport::transport_connection::BoxedConnection;
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
    RequestFailure, SessionEvent,
    modules::{
        executor,
        moqt::{
            control_plane::{
                constants::TerminationErrorCode,
                control_messages::{
                    control_message_type::ControlMessageType,
                    messages::{
                        fetch_cancel::FetchCancel, publish_done::PublishDone,
                        publish_namespace_done::PublishNamespaceDone, unsubscribe::Unsubscribe,
                        unsubscribe_namespace::UnsubscribeNamespace,
                    },
                },
                enums::{RequestId, ResponseMessage},
                error_codes::PublishDoneCode,
            },
            data_plane::stream::{
                bi_stream_sender::BiStreamSender, fetch_data_receiver::FetchDataReceiver,
            },
            runtime::dispatch::incoming_object::IncomingObject,
        },
        transport::transport_send_stream::TransportSendError,
    },
};

const CONTROL_MESSAGE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// How to withdraw a request whose success response arrives after the caller
/// stopped waiting (timeout or cancellation). Late error responses never need
/// a withdrawal and are always discarded.
#[derive(Debug)]
pub(crate) enum LateResponseAction {
    /// For TRACK_STATUS, which leaves no state at the peer.
    Discard,
    /// A late SUBSCRIBE_OK established a subscription nobody consumes.
    Unsubscribe,
    /// A late SUBSCRIBE_NAMESPACE_OK registered an unwanted namespace interest.
    UnsubscribeNamespace { namespace: Vec<String> },
    /// A late PUBLISH_NAMESPACE_OK accepted an announcement we gave up on.
    PublishNamespaceDone { namespace: Vec<String> },
    /// A late PUBLISH_OK accepted a publication nobody publishes on.
    PublishDone,
    /// A late FETCH_OK started a fetch nobody reads.
    FetchCancel,
}

/// A request awaiting its response on this session. Kept in
/// `inflight_requests` until the response arrives, even after the caller
/// stops waiting, so a late response is answered with a withdrawal instead
/// of being mistaken for an unknown Request ID (which must close the
/// session, §9.1).
pub(crate) enum InflightRequest {
    Waiting {
        sender: tokio::sync::oneshot::Sender<ResponseMessage>,
        on_late_response: LateResponseAction,
    },
    Abandoned(LateResponseAction),
}

#[derive(Default)]
pub(crate) struct InflightRequests {
    requests: HashMap<RequestId, InflightRequest>,
    control_stream_closed: bool,
}

const MAX_PENDING_OBJECTS_PER_TRACK_ALIAS: usize = 256;

pub(crate) struct SessionContext {
    pub(crate) transport_connection: BoxedConnection,
    pub(crate) send_stream: BiStreamSender,
    request_id: AtomicU64,
    track_alias: AtomicU64,
    pub(crate) event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    inflight_requests: std::sync::Mutex<InflightRequests>,
    pub(crate) receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<IncomingObject>>>,
    object_sinks: tokio::sync::Mutex<HashMap<u64, ObjectSink>>,
    pub(crate) fetch_notification_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedSender<FetchDataReceiver>>>,
    pub(crate) fetch_receiver_map:
        tokio::sync::Mutex<HashMap<u64, tokio::sync::mpsc::UnboundedReceiver<FetchDataReceiver>>>,
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

/// Holds the `inflight_requests` registration for an in-flight request. On
/// drop, a still-waiting entry is downgraded to `Abandoned` (not removed):
/// the caller gave up, but a late response must still find the entry so it
/// triggers the registered action instead of a session close.
#[must_use = "dropping this abandons the request in inflight_requests; bind it for the lifetime of the request"]
pub(crate) struct RegisteredSender {
    session: Arc<SessionContext>,
    request_id: RequestId,
}

impl Drop for RegisteredSender {
    fn drop(&mut self) {
        if let Ok(mut inflight) = self.session.inflight_requests.lock()
            && let Some(entry) = inflight.requests.get_mut(&self.request_id)
            && let InflightRequest::Waiting {
                on_late_response, ..
            } = entry
        {
            let action = std::mem::replace(on_late_response, LateResponseAction::Discard);
            *entry = InflightRequest::Abandoned(action);
        }
    }
}

impl SessionContext {
    pub(crate) fn new(
        transport_connection: BoxedConnection,
        send_stream: BiStreamSender,
        request_id: AtomicU64,
        event_sender: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        Self {
            transport_connection,
            send_stream,
            request_id,
            track_alias: AtomicU64::new(0),
            event_sender,
            inflight_requests: std::sync::Mutex::new(InflightRequests::default()),
            receiver_map: tokio::sync::Mutex::new(HashMap::new()),
            object_sinks: tokio::sync::Mutex::new(HashMap::new()),
            fetch_notification_map: tokio::sync::Mutex::new(HashMap::new()),
            fetch_receiver_map: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn get_request_id(&self) -> u64 {
        self.request_id.fetch_add(2, Ordering::SeqCst)
    }

    pub(crate) fn get_track_alias(&self) -> u64 {
        self.track_alias.fetch_add(1, Ordering::SeqCst)
    }

    /// Inserts the sender into `inflight_requests` and returns a
    /// `RegisteredSender` that marks the request abandoned on drop.
    /// `on_late_response` is the withdrawal to send if a success response
    /// arrives after abandonment. Once the control stream is closed no
    /// response can arrive, so the sender is dropped and the wait fails at
    /// once.
    pub(crate) fn register_response_sender(
        self: &Arc<Self>,
        request_id: RequestId,
        sender: tokio::sync::oneshot::Sender<ResponseMessage>,
        on_late_response: LateResponseAction,
    ) -> RegisteredSender {
        let mut inflight = self
            .inflight_requests
            .lock()
            .expect("inflight_requests poisoned");
        if !inflight.control_stream_closed {
            inflight.requests.insert(
                request_id,
                InflightRequest::Waiting {
                    sender,
                    on_late_response,
                },
            );
        }
        RegisteredSender {
            session: self.clone(),
            request_id,
        }
    }

    pub(crate) fn take_inflight_request(&self, request_id: RequestId) -> Option<InflightRequest> {
        self.inflight_requests
            .lock()
            .expect("inflight_requests poisoned")
            .requests
            .remove(&request_id)
    }

    pub(crate) fn fail_inflight_requests(&self) {
        let mut inflight = self
            .inflight_requests
            .lock()
            .expect("inflight_requests poisoned");
        inflight.control_stream_closed = true;
        inflight.requests.clear();
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

    pub(crate) async fn remove_pending_fetch(&self, request_id: RequestId) {
        self.fetch_notification_map.lock().await.remove(&request_id);
        self.fetch_receiver_map.lock().await.remove(&request_id);
    }

    /// Awaits a response to a control message with a bounded timeout (§12.2).
    ///
    /// A timeout fails only this request instead of closing the session:
    /// requests are request-scoped, and a shared (inter-relay) session must
    /// survive one slow request. §12.2's resource-exhaustion concern is met
    /// by the caller dropping its pending request state on the error path;
    /// the `inflight_requests` entry stays behind as `Abandoned` so a late
    /// response is withdrawn instead of closing the session.
    pub(crate) async fn await_response(
        &self,
        receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    ) -> Result<ResponseMessage, RequestFailure> {
        match executor::timeout(CONTROL_MESSAGE_RESPONSE_TIMEOUT, receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(RequestFailure::SessionClosed),
            Err(_) => Err(RequestFailure::Timeout),
        }
    }

    /// Handles a response that arrived after the caller abandoned the request.
    /// A success response means the peer holds state (subscription, namespace
    /// interest, ...) nobody on this side will consume, so send the matching
    /// withdrawal. A success response whose type does not match the request
    /// is a protocol violation and closes the session; error responses are
    /// simply discarded.
    pub(crate) async fn handle_late_response(
        &self,
        request_id: RequestId,
        action: LateResponseAction,
        response: ResponseMessage,
    ) {
        let send_result = match &response {
            ResponseMessage::SubscribeOk(subscribe_ok) => match &action {
                LateResponseAction::Unsubscribe => {
                    tracing::warn!(
                        request_id,
                        "SUBSCRIBE_OK arrived after the request was abandoned; sending UNSUBSCRIBE"
                    );
                    self.cancel_unclaimed_track_alias(subscribe_ok.track_alias)
                        .await;
                    self.send_stream
                        .send(
                            ControlMessageType::UnSubscribe,
                            Unsubscribe { request_id }.encode(),
                        )
                        .await
                }
                _ => self.close_on_mismatched_late_response(request_id, &response),
            },
            ResponseMessage::SubscribeNameSpaceOk(_) => match &action {
                LateResponseAction::UnsubscribeNamespace { namespace } => {
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
                _ => self.close_on_mismatched_late_response(request_id, &response),
            },
            ResponseMessage::PublishNamespaceOk(_) => match &action {
                LateResponseAction::PublishNamespaceDone { namespace } => {
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
                _ => self.close_on_mismatched_late_response(request_id, &response),
            },
            ResponseMessage::PublishOk(_) => match &action {
                LateResponseAction::PublishDone => {
                    tracing::warn!(
                        request_id,
                        "PUBLISH_OK arrived after the request was abandoned; sending PUBLISH_DONE"
                    );
                    // Stream Count 0 (§9.12): `publish()` returns no
                    // Subscription before PUBLISH_OK, so no stream was opened.
                    let publish_done = PublishDone::new(
                        request_id,
                        PublishDoneCode::InternalError.into(),
                        0,
                        "publish request abandoned".to_string(),
                    );
                    self.send_stream
                        .send(ControlMessageType::PublishDone, publish_done.encode())
                        .await
                }
                _ => self.close_on_mismatched_late_response(request_id, &response),
            },
            ResponseMessage::FetchOk(_) => match &action {
                LateResponseAction::FetchCancel => {
                    tracing::warn!(
                        request_id,
                        "FETCH_OK arrived after the request was abandoned; sending FETCH_CANCEL"
                    );
                    self.remove_pending_fetch(request_id).await;
                    self.send_stream
                        .send(
                            ControlMessageType::FetchCancel,
                            FetchCancel::new(request_id).encode(),
                        )
                        .await
                }
                _ => self.close_on_mismatched_late_response(request_id, &response),
            },
            // A TRACK_STATUS_OK for a request of another kind is discarded
            // here instead of closing the session as a mismatch.
            _ => Self::discard_late_response(request_id, &response),
        };
        if let Err(error) = send_result {
            tracing::warn!(
                request_id,
                %error,
                "failed to withdraw an abandoned request"
            );
        }
    }

    /// Nothing to withdraw (late error, or a `Discard` action): log and drop
    /// the response.
    fn discard_late_response(
        request_id: RequestId,
        response: &ResponseMessage,
    ) -> Result<(), TransportSendError> {
        tracing::warn!(
            request_id,
            ?response,
            "discarding response that arrived after the request was abandoned"
        );
        Ok(())
    }

    /// A success response whose type does not correspond to the request it
    /// answers means the peer's request-id bookkeeping cannot be trusted, so
    /// close the session instead of guessing a withdrawal.
    fn close_on_mismatched_late_response(
        &self,
        request_id: RequestId,
        response: &ResponseMessage,
    ) -> Result<(), TransportSendError> {
        tracing::error!(
            request_id,
            ?response,
            "Protocol violation: response type does not match the request; closing session"
        );
        self.close_with_error(
            TerminationErrorCode::ProtocolViolation,
            "response type does not match the request",
        );
        Ok(())
    }

    pub(crate) fn close_with_error(&self, code: TerminationErrorCode, reason: &str) {
        if let Err(error) = self.event_sender.send(SessionEvent::ProtocolViolation()) {
            tracing::error!(?error, "failed to send protocol violation event");
        }
        self.transport_connection.close(code as u32, reason);
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

    use tokio::sync::mpsc::error::TryRecvError;

    use super::{IncomingObjectNotification, LateResponseAction};
    use crate::{
        SessionEvent,
        modules::{
            moqt::{
                control_plane::{
                    control_messages::messages::{
                        fetch_ok::FetchOk,
                        parameters::{content_exists::ContentExists, filter_type::FilterType},
                        publish_ok::PublishOk,
                        subscribe_ok::SubscribeOk,
                    },
                    enums::ResponseMessage,
                },
                data_plane::stream::fetch_data_receiver::FetchDataReceiver,
                runtime::dispatch::incoming_object::IncomingObject,
            },
            test_support::{
                notify_datagram, register_and_take_data_receiver, spawn_connected_dual_sessions,
                subscription,
            },
        },
        object::{GroupOrder, Location},
        publish::PublishDoneCode,
    };

    const REQUEST_ID: u64 = 1;
    const OTHER_REQUEST_ID: u64 = 3;
    const TRACK_ALIAS: u64 = 5;

    fn late_subscribe_ok() -> ResponseMessage {
        ResponseMessage::SubscribeOk(SubscribeOk {
            request_id: REQUEST_ID,
            track_alias: TRACK_ALIAS,
            expires: 0,
            group_order: GroupOrder::Ascending,
            content_exists: ContentExists::False,
            delivery_timeout: None,
            max_duration: None,
        })
    }

    fn late_publish_ok() -> ResponseMessage {
        ResponseMessage::PublishOk(PublishOk {
            request_id: REQUEST_ID,
            forward: true,
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            filter_type: FilterType::LargestObject,
            delivery_timeout: None,
        })
    }

    fn late_fetch_ok() -> ResponseMessage {
        ResponseMessage::FetchOk(FetchOk {
            request_id: REQUEST_ID,
            group_order: GroupOrder::Ascending,
            end_of_track: false,
            end_location: Location {
                group_id: 0,
                object_id: 0,
            },
            max_cache_duration: None,
        })
    }

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
                            .map(|_| context.get_request_id())
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
            matches!(&received, Ok(IncomingObject::Datagram(object)) if object.track_alias == TRACK_ALIAS),
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
    async fn late_subscribe_ok_drops_the_objects_buffered_for_its_track_alias() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-late-subscribe-ok").await;
        let context = server.subscriber().session;
        notify_datagram(&context, TRACK_ALIAS).await;

        // Act
        context
            .handle_late_response(
                REQUEST_ID,
                LateResponseAction::Unsubscribe,
                late_subscribe_ok(),
            )
            .await;
        let notification = notify_datagram(&context, TRACK_ALIAS).await;

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::Discarded),
            "{notification:?}"
        );
        let mut receiver =
            register_and_take_data_receiver(&context, OTHER_REQUEST_ID, TRACK_ALIAS).await;
        let received = receiver.try_recv();
        assert!(
            matches!(&received, Err(TryRecvError::Empty)),
            "{received:?}"
        );
    }

    #[tokio::test]
    async fn late_subscribe_ok_keeps_the_receiver_of_another_subscription() {
        // Arrange
        let (_client, server) =
            spawn_connected_dual_sessions("sinks-late-subscribe-ok-other").await;
        let context = server.subscriber().session;
        context
            .register_data_receiver(OTHER_REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();

        // Act
        context
            .handle_late_response(
                REQUEST_ID,
                LateResponseAction::Unsubscribe,
                late_subscribe_ok(),
            )
            .await;
        let notification = notify_datagram(&context, TRACK_ALIAS).await;

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::Notified),
            "{notification:?}"
        );
    }

    #[tokio::test]
    async fn late_publish_ok_is_withdrawn_with_publish_done() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("late-publish-ok").await;
        let context = server.publisher().session;

        // Act
        context
            .handle_late_response(
                REQUEST_ID,
                LateResponseAction::PublishDone,
                late_publish_ok(),
            )
            .await;

        // Assert
        let SessionEvent::PublishDone(publish_done) = client.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_DONE");
        };
        assert_eq!(publish_done.request_id(), REQUEST_ID);
        assert_eq!(publish_done.status_code(), PublishDoneCode::InternalError);
        assert_eq!(publish_done.stream_count(), 0);
    }

    #[tokio::test]
    async fn late_fetch_ok_is_withdrawn_with_fetch_cancel() {
        // Arrange: the fetch future was dropped, so its maps were never cleaned up
        let (client, server) = spawn_connected_dual_sessions("late-fetch-ok").await;
        let context = server.subscriber().session;
        let (fetch_stream_sender, fetch_stream_receiver) =
            tokio::sync::mpsc::unbounded_channel::<FetchDataReceiver>();
        context
            .fetch_notification_map
            .lock()
            .await
            .insert(REQUEST_ID, fetch_stream_sender);
        context
            .fetch_receiver_map
            .lock()
            .await
            .insert(REQUEST_ID, fetch_stream_receiver);

        // Act
        context
            .handle_late_response(REQUEST_ID, LateResponseAction::FetchCancel, late_fetch_ok())
            .await;

        // Assert: FETCH_CANCEL is sent and the pending fetch state is dropped
        let SessionEvent::FetchCancel(fetch_cancel) = client.receive_event().await.unwrap() else {
            panic!("expected FETCH_CANCEL");
        };
        assert_eq!(fetch_cancel.request_id(), REQUEST_ID);
        assert!(
            !context
                .fetch_notification_map
                .lock()
                .await
                .contains_key(&REQUEST_ID)
        );
        assert!(
            !context
                .fetch_receiver_map
                .lock()
                .await
                .contains_key(&REQUEST_ID)
        );
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

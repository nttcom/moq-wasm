//! draft-14 carries every control message on one bidirectional control
//! stream and pairs a response with its request by Request ID (§9.1).

pub(crate) mod control_message_receive_task;
pub(crate) mod error_codes;
pub(crate) mod objects;
pub(crate) mod setup;

use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use bytes::BytesMut;

use crate::{
    RequestFailure,
    modules::{
        executor,
        moqt::{
            control_plane::{
                constants::TerminationErrorCode,
                control_messages::{
                    control_message_type::ControlMessageType,
                    messages::{
                        fetch_cancel::FetchCancel, publish_done::PublishDone,
                        publish_namespace_cancel::PublishNamespaceCancel,
                        publish_namespace_done::PublishNamespaceDone, request_error::RequestError,
                        requests_blocked::RequestsBlocked, subscribe_update::SubscribeUpdate,
                        unsubscribe::Unsubscribe, unsubscribe_namespace::UnsubscribeNamespace,
                    },
                },
                enums::{RequestId, ResponseMessage},
                error_codes::{PublishDoneCode, RequestKind},
            },
            data_plane::stream::bi_stream_sender::BiStreamSender,
            domains::{connection::Connection, session_context::SessionContext},
            version::{
                self, Notification, OutgoingRequest, OutgoingResponse, RequestIdReservation,
            },
        },
        transport::transport_send_stream::TransportSendError,
    },
};

const CONTROL_MESSAGE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// The SETUP MAX_REQUEST_ID this side sends, and how far past the peer's next
/// Request ID every later MAX_REQUEST_ID reaches. Request IDs step by 2, so it
/// allows 500 requests.
pub(super) const REQUEST_ID_WINDOW: RequestId = 1000;

/// How to withdraw a request whose success response the caller abandons: by
/// timing out, by cancelling, or by dropping the `ReceivedResponse` before
/// `accept`. Error responses need no withdrawal and are discarded.
#[derive(Debug)]
enum LateResponseAction {
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
enum InflightRequest {
    Waiting {
        sender: tokio::sync::oneshot::Sender<ReceivedResponse>,
        on_late_response: LateResponseAction,
    },
    Abandoned(LateResponseAction),
}

#[derive(Default)]
struct InflightRequests {
    requests: HashMap<RequestId, InflightRequest>,
    control_stream_closed: bool,
}

struct OutgoingRequestIds {
    next: RequestId,
    /// The peer's Maximum Request ID this side last sent REQUESTS_BLOCKED for.
    blocked_on: Option<RequestId>,
}

pub(super) struct RequestIdViolation {
    pub(super) code: TerminationErrorCode,
    pub(super) reason: &'static str,
}

pub(crate) struct ControlChannel {
    send_stream: BiStreamSender,
    request_ids: tokio::sync::Mutex<OutgoingRequestIds>,
    /// Exclusive upper bound for this side's Request IDs: the peer's SETUP or
    /// MAX_REQUEST_ID (§9.5).
    peer_request_id_limit: tokio::sync::watch::Sender<RequestId>,
    /// The smallest Request ID the peer may use for its next request.
    next_peer_request_id: AtomicU64,
    /// Exclusive upper bound for the peer's Request IDs: the last SETUP or
    /// MAX_REQUEST_ID this side sent.
    request_id_limit: AtomicU64,
    inflight_requests: Arc<std::sync::Mutex<InflightRequests>>,
}

impl ControlChannel {
    /// A client numbers its requests from 0 and a server from 1, each in
    /// steps of two. `peer_request_id_limit` is the peer's SETUP
    /// MAX_REQUEST_ID; its default 0 allows no request (§9.3.2.3).
    fn new(
        send_stream: BiStreamSender,
        first_request_id: RequestId,
        peer_request_id_limit: RequestId,
    ) -> Self {
        Self {
            send_stream,
            request_ids: tokio::sync::Mutex::new(OutgoingRequestIds {
                next: first_request_id,
                blocked_on: None,
            }),
            peer_request_id_limit: tokio::sync::watch::Sender::new(peer_request_id_limit),
            next_peer_request_id: AtomicU64::new(first_request_id ^ 1),
            request_id_limit: AtomicU64::new(REQUEST_ID_WINDOW),
            inflight_requests: Arc::default(),
        }
    }

    fn of(connection: &Connection) -> &Self {
        let version::ControlChannel::Draft14(channel) = &connection.control;
        channel
    }

    /// Waits while the next Request ID reaches the peer's Maximum Request
    /// ID, sending one REQUESTS_BLOCKED per limit (§9.6).
    pub(super) async fn reserve_request_id(&self) -> RequestIdReservation<'_> {
        let mut request_ids = self.request_ids.lock().await;
        let mut peer_request_id_limit = self.peer_request_id_limit.subscribe();
        loop {
            let limit = *peer_request_id_limit.borrow_and_update();
            if request_ids.next < limit {
                break;
            }
            if request_ids.blocked_on != Some(limit) {
                request_ids.blocked_on = Some(limit);
                tracing::debug!(
                    maximum_request_id = limit,
                    "requests blocked by the peer's Maximum Request ID"
                );
                let blocked = Notification::RequestsBlocked(RequestsBlocked::new(limit));
                if let Err(error) = self.notify(blocked).await {
                    tracing::debug!(%error, "failed to send REQUESTS_BLOCKED");
                }
            }
            // `self` owns the sender, so this only returns on a new value.
            let _ = peer_request_id_limit.changed().await;
        }
        RequestIdReservation(tokio::sync::MutexGuard::map(request_ids, |ids| {
            &mut ids.next
        }))
    }

    /// Returns the Maximum Request ID to advertise once the peer has used half
    /// the window, so a peer that keeps sending never blocks. Fails when the
    /// Request ID reuses or goes below an earlier one, has this side's parity
    /// (§9.1), or reaches the advertised limit (§9.5).
    fn accept_peer_request_id(
        &self,
        request_id: RequestId,
    ) -> Result<Option<RequestId>, RequestIdViolation> {
        let invalid = |reason| RequestIdViolation {
            code: TerminationErrorCode::InvalidRequestId,
            reason,
        };
        let next = self.next_peer_request_id.load(Ordering::Relaxed);
        if request_id % 2 != next % 2 {
            return Err(invalid("Request ID has the parity of the other endpoint"));
        }
        if request_id < next {
            return Err(invalid(
                "Request ID is not larger than the previous request's",
            ));
        }
        let limit = self.request_id_limit.load(Ordering::Relaxed);
        if request_id >= limit {
            return Err(RequestIdViolation {
                code: TerminationErrorCode::TooManyRequests,
                reason: "Request ID reaches the Maximum Request ID",
            });
        }
        let next = request_id + 2;
        self.next_peer_request_id.store(next, Ordering::Relaxed);
        if limit.saturating_sub(next) >= REQUEST_ID_WINDOW / 2 {
            return Ok(None);
        }
        let raised = next + REQUEST_ID_WINDOW;
        self.request_id_limit.store(raised, Ordering::Relaxed);
        Ok(Some(raised))
    }

    /// Fails unless `limit` is above the current one (§9.5).
    fn raise_peer_request_id_limit(&self, limit: RequestId) -> Result<(), &'static str> {
        let raised = self.peer_request_id_limit.send_if_modified(|current| {
            let raised = limit > *current;
            if raised {
                *current = limit;
            }
            raised
        });
        raised
            .then_some(())
            .ok_or("MAX_REQUEST_ID does not increase the Maximum Request ID")
    }

    /// Writes the request before releasing `reservation`, so the peer
    /// receives this side's Request IDs in increasing order.
    pub(super) async fn send_request(
        &self,
        reservation: RequestIdReservation<'_>,
        request: OutgoingRequest,
    ) -> Result<PendingResponse, RequestFailure> {
        let (request_id, message_type, payload, on_late_response) = match request {
            OutgoingRequest::Subscribe(subscribe) => (
                subscribe.request_id,
                ControlMessageType::Subscribe,
                subscribe.encode(),
                LateResponseAction::Unsubscribe,
            ),
            OutgoingRequest::TrackStatus(track_status) => (
                track_status.request_id,
                ControlMessageType::TrackStatus,
                track_status.encode(),
                LateResponseAction::Discard,
            ),
            OutgoingRequest::Fetch(fetch) => (
                fetch.request_id,
                ControlMessageType::Fetch,
                fetch.encode(),
                LateResponseAction::FetchCancel,
            ),
            OutgoingRequest::Publish(publish) => (
                publish.request_id,
                ControlMessageType::Publish,
                publish.encode(),
                LateResponseAction::PublishDone,
            ),
            OutgoingRequest::PublishNamespace(publish_namespace) => (
                publish_namespace.request_id,
                ControlMessageType::PublishNamespace,
                publish_namespace.encode(),
                LateResponseAction::PublishNamespaceDone {
                    namespace: publish_namespace.track_namespace,
                },
            ),
            OutgoingRequest::SubscribeNamespace(subscribe_namespace) => (
                subscribe_namespace.request_id,
                ControlMessageType::SubscribeNamespace,
                subscribe_namespace.encode(),
                LateResponseAction::UnsubscribeNamespace {
                    namespace: subscribe_namespace.track_namespace_prefix,
                },
            ),
        };
        debug_assert_eq!(request_id, reservation.request_id());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let registered = self.register(request_id, sender, on_late_response);
        self.send_with_request_id(reservation, message_type, payload)
            .await?;
        Ok(PendingResponse {
            receiver,
            _registered: registered,
        })
    }

    pub(super) async fn send_subscribe_update(
        &self,
        reservation: RequestIdReservation<'_>,
        subscribe_update: SubscribeUpdate,
    ) -> Result<(), TransportSendError> {
        debug_assert_eq!(subscribe_update.request_id, reservation.request_id());
        self.send_with_request_id(
            reservation,
            ControlMessageType::SubscribeUpdate,
            subscribe_update.encode(),
        )
        .await
    }

    async fn send_with_request_id(
        &self,
        reservation: RequestIdReservation<'_>,
        message_type: ControlMessageType,
        payload: BytesMut,
    ) -> Result<(), TransportSendError> {
        let RequestIdReservation(mut next_request_id) = reservation;
        *next_request_id += 2;
        self.send_stream.send(message_type, payload).await
    }

    pub(super) async fn respond(
        &self,
        response: OutgoingResponse,
    ) -> Result<(), TransportSendError> {
        let (message_type, payload) = match response {
            OutgoingResponse::SubscribeOk(ok) => (ControlMessageType::SubscribeOk, ok.encode()),
            OutgoingResponse::TrackStatusOk(ok) => (ControlMessageType::TrackStatusOk, ok.encode()),
            OutgoingResponse::PublishOk(ok) => (ControlMessageType::PublishOk, ok.encode()),
            OutgoingResponse::FetchOk(ok) => (ControlMessageType::FetchOk, ok.encode()),
            OutgoingResponse::PublishNamespaceOk(ok) => {
                (ControlMessageType::PublishNamespaceOk, ok.encode())
            }
            OutgoingResponse::SubscribeNamespaceOk(ok) => {
                (ControlMessageType::SubscribeNamespaceOk, ok.encode())
            }
            OutgoingResponse::Error {
                kind,
                request_id,
                code,
                reason_phrase,
            } => {
                let error = RequestError {
                    request_id,
                    error_code: error_codes::request_error_code_to_wire(code, kind),
                    reason_phrase,
                };
                (error_message_type(kind), error.encode())
            }
        };
        self.send_stream.send(message_type, payload).await
    }

    pub(super) async fn notify(
        &self,
        notification: Notification,
    ) -> Result<(), TransportSendError> {
        let (message_type, payload) = match notification {
            Notification::Unsubscribe(m) => (ControlMessageType::UnSubscribe, m.encode()),
            Notification::PublishDone {
                request_id,
                code,
                stream_count,
                reason,
            } => {
                let publish_done = PublishDone::new(
                    request_id,
                    error_codes::publish_done_code_to_wire(code),
                    stream_count,
                    reason,
                );
                (ControlMessageType::PublishDone, publish_done.encode())
            }
            Notification::FetchCancel(m) => (ControlMessageType::FetchCancel, m.encode()),
            Notification::PublishNamespaceDone(m) => {
                (ControlMessageType::PublishNamespaceDone, m.encode())
            }
            Notification::PublishNamespaceCancel {
                namespace,
                code,
                reason,
            } => {
                // §9.27: PUBLISH_NAMESPACE_CANCEL uses the PUBLISH_NAMESPACE_ERROR codes.
                let cancel = PublishNamespaceCancel::new(
                    namespace,
                    error_codes::request_error_code_to_wire(code, RequestKind::PublishNamespace),
                    reason,
                );
                (ControlMessageType::PublishNamespaceCancel, cancel.encode())
            }
            Notification::UnsubscribeNamespace(m) => {
                (ControlMessageType::UnSubscribeNamespace, m.encode())
            }
            Notification::GoAway(m) => (ControlMessageType::GoAway, m.encode()),
            Notification::MaxRequestId(m) => (ControlMessageType::MaxRequestId, m.encode()),
            Notification::RequestsBlocked(m) => (ControlMessageType::RequestsBlocked, m.encode()),
        };
        self.send_stream.send(message_type, payload).await
    }

    /// `on_late_response` is the withdrawal to send if a success response arrives after abandonment.
    /// Once the control stream is closed no response can arrive, so the
    /// sender is dropped and the wait fails at once.
    fn register(
        &self,
        request_id: RequestId,
        sender: tokio::sync::oneshot::Sender<ReceivedResponse>,
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
            inflight_requests: self.inflight_requests.clone(),
            request_id,
        }
    }

    fn take_inflight_request(&self, request_id: RequestId) -> Option<InflightRequest> {
        self.inflight_requests
            .lock()
            .expect("inflight_requests poisoned")
            .requests
            .remove(&request_id)
    }

    fn fail_inflight_requests(&self) {
        let mut inflight = self
            .inflight_requests
            .lock()
            .expect("inflight_requests poisoned");
        inflight.control_stream_closed = true;
        inflight.requests.clear();
        drop(inflight);
        // No MAX_REQUEST_ID can arrive anymore: release blocked requests so
        // they fail like any request on a closed control stream.
        self.peer_request_id_limit.send_replace(RequestId::MAX);
    }
}

fn error_message_type(kind: RequestKind) -> ControlMessageType {
    match kind {
        RequestKind::Subscribe => ControlMessageType::SubscribeError,
        RequestKind::Publish => ControlMessageType::PublishError,
        RequestKind::Fetch => ControlMessageType::FetchError,
        RequestKind::TrackStatus => ControlMessageType::TrackStatusError,
        RequestKind::PublishNamespace => ControlMessageType::PublishNamespaceError,
        RequestKind::SubscribeNamespace => ControlMessageType::SubscribeNamespaceError,
    }
}

/// Holds the `inflight_requests` registration for an in-flight request. On
/// drop, a still-waiting entry is downgraded to `Abandoned` (not removed):
/// the caller gave up, but a late response must still find the entry so it
/// triggers the registered action instead of a session close.
struct RegisteredSender {
    inflight_requests: Arc<std::sync::Mutex<InflightRequests>>,
    request_id: RequestId,
}

impl Drop for RegisteredSender {
    fn drop(&mut self) {
        if let Ok(mut inflight) = self.inflight_requests.lock()
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

pub(crate) struct PendingResponse {
    receiver: tokio::sync::oneshot::Receiver<ReceivedResponse>,
    _registered: RegisteredSender,
}

impl PendingResponse {
    /// Bounds the wait (§12.2). A timeout fails only this request, so a shared
    /// (inter-relay) session survives one slow request; the entry stays
    /// `Abandoned` and a late response goes to `handle_late_response`.
    pub(super) async fn response(self) -> Result<ReceivedResponse, RequestFailure> {
        match executor::timeout(CONTROL_MESSAGE_RESPONSE_TIMEOUT, self.receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(RequestFailure::SessionClosed),
            Err(_) => Err(RequestFailure::Timeout),
        }
    }
}

/// A response on its way to the caller. Dropped before `accept`, while still
/// in the channel or after the caller got it, it is withdrawn through
/// `handle_late_response`.
pub(crate) struct ReceivedResponse(Option<UnacceptedResponse>);

struct UnacceptedResponse {
    message: ResponseMessage,
    session: Weak<SessionContext>,
    connection: Weak<Connection>,
    request_id: RequestId,
    action: LateResponseAction,
}

impl ReceivedResponse {
    fn new(
        message: ResponseMessage,
        session: &Arc<SessionContext>,
        connection: &Arc<Connection>,
        request_id: RequestId,
        action: LateResponseAction,
    ) -> Self {
        Self(Some(UnacceptedResponse {
            message,
            session: Arc::downgrade(session),
            connection: Arc::downgrade(connection),
            request_id,
            action,
        }))
    }

    // Only `accept` and `drop` take the response, and both consume `self`.
    pub(super) fn message(&self) -> &ResponseMessage {
        &self.0.as_ref().expect("taken only on consume").message
    }

    pub(super) fn accept(mut self) -> ResponseMessage {
        self.0.take().expect("taken only on consume").message
    }
}

impl Drop for ReceivedResponse {
    fn drop(&mut self) {
        let Some(unaccepted) = self.0.take() else {
            return;
        };
        let (Some(session), Some(connection)) = (
            unaccepted.session.upgrade(),
            unaccepted.connection.upgrade(),
        ) else {
            return;
        };
        // A single fire-and-forget withdrawal needs no owned JoinHandle.
        executor::try_spawn("Abandoned Response Withdrawal", async move {
            handle_late_response(
                &session,
                &connection,
                unaccepted.request_id,
                unaccepted.action,
                unaccepted.message,
            )
            .await;
        });
    }
}

/// Handles a response the caller abandoned, before or after it arrived.
/// A success response means the peer holds state (subscription, namespace
/// interest, ...) nobody on this side will consume, so send the matching
/// withdrawal. A success response whose type does not match the request is a
/// protocol violation and closes the session; error responses are simply
/// discarded.
async fn handle_late_response(
    session: &SessionContext,
    connection: &Connection,
    request_id: RequestId,
    action: LateResponseAction,
    response: ResponseMessage,
) {
    let channel = ControlChannel::of(connection);
    let send_result = match &response {
        ResponseMessage::SubscribeOk(subscribe_ok) => match &action {
            LateResponseAction::Unsubscribe => {
                tracing::warn!(
                    request_id,
                    "SUBSCRIBE_OK of an abandoned request; sending UNSUBSCRIBE"
                );
                session
                    .cancel_unclaimed_track_alias(connection, subscribe_ok.track_alias)
                    .await;
                // The caller may have registered its data receiver before dropping the response.
                session.cancel_subscription(connection, request_id).await;
                channel
                    .notify(Notification::Unsubscribe(Unsubscribe { request_id }))
                    .await
            }
            _ => close_on_mismatched_late_response(session, connection, request_id, &response),
        },
        ResponseMessage::SubscribeNameSpaceOk(_) => match &action {
            LateResponseAction::UnsubscribeNamespace { namespace } => {
                tracing::warn!(
                    request_id,
                    "SUBSCRIBE_NAMESPACE_OK of an abandoned request; sending UNSUBSCRIBE_NAMESPACE"
                );
                channel
                    .notify(Notification::UnsubscribeNamespace(
                        UnsubscribeNamespace::new(namespace.clone()),
                    ))
                    .await
            }
            _ => close_on_mismatched_late_response(session, connection, request_id, &response),
        },
        ResponseMessage::PublishNamespaceOk(_) => match &action {
            LateResponseAction::PublishNamespaceDone { namespace } => {
                tracing::warn!(
                    request_id,
                    "PUBLISH_NAMESPACE_OK of an abandoned request; sending PUBLISH_NAMESPACE_DONE"
                );
                channel
                    .notify(Notification::PublishNamespaceDone(
                        PublishNamespaceDone::new(namespace.clone()),
                    ))
                    .await
            }
            _ => close_on_mismatched_late_response(session, connection, request_id, &response),
        },
        ResponseMessage::PublishOk(_) => match &action {
            LateResponseAction::PublishDone => {
                tracing::warn!(
                    request_id,
                    "PUBLISH_OK of an abandoned request; sending PUBLISH_DONE"
                );
                // Stream Count 0 (§9.12): `publish()` returns no
                // Subscription before it accepts PUBLISH_OK, so no stream
                // was opened.
                channel
                    .notify(Notification::PublishDone {
                        request_id,
                        code: PublishDoneCode::InternalError,
                        stream_count: 0,
                        reason: "publish request abandoned".to_string(),
                    })
                    .await
            }
            _ => close_on_mismatched_late_response(session, connection, request_id, &response),
        },
        ResponseMessage::FetchOk(_) => match &action {
            LateResponseAction::FetchCancel => {
                tracing::warn!(
                    request_id,
                    "FETCH_OK of an abandoned request; sending FETCH_CANCEL"
                );
                channel
                    .notify(Notification::FetchCancel(FetchCancel::new(request_id)))
                    .await
            }
            _ => close_on_mismatched_late_response(session, connection, request_id, &response),
        },
        ResponseMessage::TrackStatusOk(_) => match &action {
            LateResponseAction::Discard => discard_late_response(request_id, &response),
            _ => close_on_mismatched_late_response(session, connection, request_id, &response),
        },
        _ => discard_late_response(request_id, &response),
    };
    if let Err(error) = send_result {
        tracing::warn!(
            request_id,
            %error,
            "failed to withdraw an abandoned request"
        );
    }
}

/// Nothing to withdraw (late error, or a `Discard` action): log and drop the
/// response.
fn discard_late_response(
    request_id: RequestId,
    response: &ResponseMessage,
) -> Result<(), TransportSendError> {
    tracing::warn!(
        request_id,
        ?response,
        "discarding the response of an abandoned request"
    );
    Ok(())
}

/// A success response whose type does not correspond to the request it
/// answers means the peer's request-id bookkeeping cannot be trusted, so
/// close the session instead of guessing a withdrawal.
fn close_on_mismatched_late_response(
    session: &SessionContext,
    connection: &Connection,
    request_id: RequestId,
    response: &ResponseMessage,
) -> Result<(), TransportSendError> {
    tracing::error!(
        request_id,
        ?response,
        "Protocol violation: response type does not match the request; closing session"
    );
    session.close_connection(
        connection,
        TerminationErrorCode::ProtocolViolation,
        "response type does not match the request",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{pin::Pin, sync::Arc, time::Duration};

    use tokio::sync::mpsc::error::TryRecvError;

    use super::{
        ControlChannel, LateResponseAction, Notification, REQUEST_ID_WINDOW, RequestIdReservation,
        handle_late_response,
    };
    use crate::{
        Session, SessionEvent,
        modules::{
            moqt::{
                control_plane::{
                    control_messages::control_message_type::ControlMessageType,
                    control_messages::messages::{
                        fetch_ok::FetchOk,
                        max_request_id::MaxRequestId,
                        parameters::{content_exists::ContentExists, filter_type::FilterType},
                        publish_ok::PublishOk,
                        subscribe::Subscribe,
                        subscribe_ok::SubscribeOk,
                    },
                    enums::ResponseMessage,
                },
                domains::connection::{Connection, IncomingObjectNotification},
            },
            test_support::{
                DISCONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, notify_datagram,
                register_and_take_data_receiver, spawn_connected_dual_sessions,
            },
        },
        object::{GroupOrder, Location},
        publish::PublishDoneCode,
        subscribe::TrackStatusHandler,
    };

    const REQUEST_ID: u64 = 1;
    const OTHER_REQUEST_ID: u64 = 3;
    const TRACK_ALIAS: u64 = 5;

    fn subscribe_ok() -> SubscribeOk {
        SubscribeOk {
            request_id: REQUEST_ID,
            track_alias: TRACK_ALIAS,
            expires: 0,
            group_order: GroupOrder::Ascending,
            content_exists: ContentExists::False,
            delivery_timeout: None,
            max_duration: None,
        }
    }

    fn late_subscribe_ok() -> ResponseMessage {
        ResponseMessage::SubscribeOk(subscribe_ok())
    }

    fn late_track_status_ok() -> ResponseMessage {
        ResponseMessage::TrackStatusOk(subscribe_ok())
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
    async fn late_subscribe_ok_drops_the_objects_buffered_for_its_track_alias() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("sinks-late-subscribe-ok").await;
        let context = server.subscriber().session;
        notify_datagram(&context, TRACK_ALIAS).await;

        // Act
        handle_late_response(
            &context,
            &context.connection(),
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
            .register_data_receiver(&context.connection(), OTHER_REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();

        // Act
        handle_late_response(
            &context,
            &context.connection(),
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
        handle_late_response(
            &context,
            &context.connection(),
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
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("late-fetch-ok").await;
        let context = server.subscriber().session;

        // Act
        handle_late_response(
            &context,
            &context.connection(),
            REQUEST_ID,
            LateResponseAction::FetchCancel,
            late_fetch_ok(),
        )
        .await;

        // Assert
        let SessionEvent::FetchCancel(fetch_cancel) = client.receive_event().await.unwrap() else {
            panic!("expected FETCH_CANCEL");
        };
        assert_eq!(fetch_cancel.request_id(), REQUEST_ID);
    }

    #[tokio::test]
    async fn late_track_status_ok_for_another_request_kind_closes_the_session() {
        // Arrange
        let (_client, server) = spawn_connected_dual_sessions("late-track-status-ok").await;
        let context = server.subscriber().session;

        // Act
        handle_late_response(
            &context,
            &context.connection(),
            REQUEST_ID,
            LateResponseAction::Unsubscribe,
            late_track_status_ok(),
        )
        .await;

        // Assert
        let event = tokio::time::timeout(HANDSHAKE_TIMEOUT, server.receive_event())
            .await
            .expect("no session event")
            .unwrap();
        assert!(
            matches!(event, SessionEvent::ProtocolViolation()),
            "{event:?}"
        );
    }

    /// Bypasses the Request ID reservation.
    async fn send_track_status(session: &Session, request_id: u64) {
        let track_status = Subscribe {
            request_id,
            track_namespace: vec!["ns".to_string()],
            track_name: "track".to_string(),
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: false,
            filter_type: FilterType::LargestObject,
            authorization_tokens: vec![],
            delivery_timeout: None,
        };
        let connection = session.publisher().session.connection();
        ControlChannel::of(&connection)
            .send_stream
            .send(ControlMessageType::TrackStatus, track_status.encode())
            .await
            .unwrap();
    }

    async fn assert_closed_with(session: &Session, code: u32, reason: &str) {
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, session.closed())
            .await
            .unwrap();
        assert_eq!(close.code, Some(code));
        assert_eq!(close.reason, reason);
    }

    async fn assert_closed_with_invalid_request_id(session: &Session, reason: &str) {
        assert_closed_with(session, 0x4, reason).await;
    }

    fn connection_of(session: &Session) -> Arc<Connection> {
        session.publisher().session.connection()
    }

    async fn blocked_reservation(
        channel: &ControlChannel,
    ) -> Pin<Box<impl Future<Output = RequestIdReservation<'_>>>> {
        channel.peer_request_id_limit.send_replace(0);
        let mut reservation = Box::pin(channel.reserve_request_id());
        let blocked = tokio::time::timeout(Duration::from_millis(100), &mut reservation).await;
        assert!(blocked.is_err(), "reserved past the peer's limit");
        reservation
    }

    /// The handler is returned so its drop does not answer a request the
    /// test sent outside the client's inflight requests.
    async fn receive_track_status(session: &Session) -> TrackStatusHandler {
        let event = tokio::time::timeout(HANDSHAKE_TIMEOUT, session.receive_event())
            .await
            .unwrap()
            .unwrap();
        let SessionEvent::TrackStatus(handler) = event else {
            panic!("expected TRACK_STATUS, got {event:?}");
        };
        handler
    }

    #[tokio::test]
    async fn request_id_with_the_receivers_parity_closes_the_session() {
        // Arrange
        let (client, _server) = spawn_connected_dual_sessions("request-id-wrong-parity").await;

        // Act
        send_track_status(&client, 1).await;

        // Assert: the server closes the session
        assert_closed_with_invalid_request_id(
            &client,
            "Request ID has the parity of the other endpoint",
        )
        .await;
    }

    #[tokio::test]
    async fn reused_request_id_closes_the_session() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("request-id-reused").await;
        send_track_status(&client, 0).await;
        let first = receive_track_status(&server).await;
        assert_eq!(first.request_id(), 0);

        // Act
        send_track_status(&client, 0).await;

        // Assert: the server closes the session
        assert_closed_with_invalid_request_id(
            &client,
            "Request ID is not larger than the previous request's",
        )
        .await;
    }

    #[tokio::test]
    async fn request_id_skipping_unused_ids_is_accepted() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("request-id-gap").await;
        send_track_status(&client, 0).await;
        let first = receive_track_status(&server).await;
        assert_eq!(first.request_id(), 0);

        // Act
        send_track_status(&client, 6).await;

        // Assert
        assert_eq!(receive_track_status(&server).await.request_id(), 6);
    }

    #[tokio::test]
    async fn request_id_check_applies_to_requests_from_the_server() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("request-id-server-parity").await;

        // Act
        send_track_status(&server, 0).await;

        // Assert: the client closes the session
        assert_closed_with_invalid_request_id(
            &server,
            "Request ID has the parity of the other endpoint",
        )
        .await;
        drop(client);
    }

    #[tokio::test]
    async fn request_id_at_the_maximum_request_id_closes_the_session() {
        // Arrange
        let (client, _server) = spawn_connected_dual_sessions("request-id-too-many").await;

        // Act
        send_track_status(&client, REQUEST_ID_WINDOW).await;

        // Assert: the server closes the session with TOO_MANY_REQUESTS
        assert_closed_with(&client, 0x7, "Request ID reaches the Maximum Request ID").await;
    }

    #[tokio::test]
    async fn peer_past_half_the_window_gets_a_raised_maximum_request_id() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("request-id-raised").await;
        let mut client_limit = ControlChannel::of(&connection_of(&client))
            .peer_request_id_limit
            .subscribe();

        // Act
        send_track_status(&client, REQUEST_ID_WINDOW / 2).await;
        let _first = receive_track_status(&server).await;

        // Assert: the limit moves a window past the client's next Request ID
        let raised = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            client_limit.wait_for(|limit| *limit != REQUEST_ID_WINDOW),
        )
        .await
        .unwrap()
        .map(|limit| *limit)
        .unwrap();
        assert_eq!(raised, REQUEST_ID_WINDOW / 2 + 2 + REQUEST_ID_WINDOW);
        send_track_status(&client, REQUEST_ID_WINDOW).await;
        assert_eq!(
            receive_track_status(&server).await.request_id(),
            REQUEST_ID_WINDOW
        );
    }

    #[tokio::test]
    async fn max_request_id_that_does_not_increase_closes_the_session() {
        // Arrange
        let (client, _server) = spawn_connected_dual_sessions("max-request-id-not-raised").await;

        // Act
        ControlChannel::of(&connection_of(&client))
            .send_stream
            .send(
                ControlMessageType::MaxRequestId,
                MaxRequestId::new(REQUEST_ID_WINDOW).encode(),
            )
            .await
            .unwrap();

        // Assert: the server closes the session with PROTOCOL_VIOLATION
        assert_closed_with(
            &client,
            0x3,
            "MAX_REQUEST_ID does not increase the Maximum Request ID",
        )
        .await;
    }

    #[tokio::test]
    async fn request_at_the_peers_limit_waits_for_max_request_id() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("request-id-blocked").await;
        let client_connection = connection_of(&client);
        let client_channel = ControlChannel::of(&client_connection);
        let reservation = blocked_reservation(client_channel).await;

        // Act
        ControlChannel::of(&connection_of(&server))
            .notify(Notification::MaxRequestId(MaxRequestId::new(2)))
            .await
            .unwrap();

        // Assert
        let reservation = tokio::time::timeout(HANDSHAKE_TIMEOUT, reservation)
            .await
            .unwrap();
        assert_eq!(reservation.request_id(), 0);
        drop(reservation);
        assert_eq!(client_channel.request_ids.lock().await.blocked_on, Some(0));
    }

    #[tokio::test]
    async fn closed_control_stream_releases_a_blocked_request() {
        // Arrange
        let (client, server) = spawn_connected_dual_sessions("request-id-blocked-closed").await;
        let client_connection = connection_of(&client);
        let client_channel = ControlChannel::of(&client_connection);
        let reservation = blocked_reservation(client_channel).await;

        // Act
        drop(server);

        // Assert
        let reservation = tokio::time::timeout(DISCONNECT_TIMEOUT, reservation)
            .await
            .unwrap();
        assert_eq!(reservation.request_id(), 0);
    }
}

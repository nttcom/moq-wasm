//! draft-14 carries every control message on one bidirectional control
//! stream and pairs a response with its request by Request ID (§9.1).

pub(crate) mod control_message_receive_task;
pub(crate) mod error_codes;
pub(crate) mod objects;
pub(crate) mod setup;

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

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
                        unsubscribe::Unsubscribe, unsubscribe_namespace::UnsubscribeNamespace,
                    },
                },
                enums::{RequestId, ResponseMessage},
                error_codes::{PublishDoneCode, RequestKind},
            },
            data_plane::stream::bi_stream_sender::BiStreamSender,
            domains::{connection::Connection, session_context::SessionContext},
            version::{self, Notification, OutgoingRequest, OutgoingResponse},
        },
        transport::transport_send_stream::TransportSendError,
    },
};

const CONTROL_MESSAGE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// How to withdraw a request whose success response arrives after the caller
/// stopped waiting (timeout or cancellation). Late error responses never need
/// a withdrawal and are always discarded.
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
        sender: tokio::sync::oneshot::Sender<ResponseMessage>,
        on_late_response: LateResponseAction,
    },
    Abandoned(LateResponseAction),
}

#[derive(Default)]
struct InflightRequests {
    requests: HashMap<RequestId, InflightRequest>,
    control_stream_closed: bool,
}

pub(crate) struct ControlChannel {
    send_stream: BiStreamSender,
    request_id: AtomicU64,
    inflight_requests: Arc<std::sync::Mutex<InflightRequests>>,
}

impl ControlChannel {
    /// A client numbers its requests from 0 and a server from 1, each in
    /// steps of two.
    fn new(send_stream: BiStreamSender, first_request_id: RequestId) -> Self {
        Self {
            send_stream,
            request_id: AtomicU64::new(first_request_id),
            inflight_requests: Arc::default(),
        }
    }

    fn of(connection: &Connection) -> &Self {
        let version::ControlChannel::Draft14(channel) = &connection.control;
        channel
    }

    pub(crate) fn next_request_id(&self) -> RequestId {
        self.request_id.fetch_add(2, Ordering::SeqCst)
    }

    pub(super) async fn send_request(
        &self,
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
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let registered = self.register(request_id, sender, on_late_response);
        self.send_stream.send(message_type, payload).await?;
        Ok(PendingResponse {
            receiver,
            _registered: registered,
        })
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
            Notification::SubscribeUpdate(m) => (ControlMessageType::SubscribeUpdate, m.encode()),
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
    receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    _registered: RegisteredSender,
}

impl PendingResponse {
    /// Bounds the wait (§12.2). A timeout fails only this request, so a shared
    /// (inter-relay) session survives one slow request; the entry stays
    /// `Abandoned` and a late response goes to `handle_late_response`.
    pub(super) async fn response(self) -> Result<ResponseMessage, RequestFailure> {
        match executor::timeout(CONTROL_MESSAGE_RESPONSE_TIMEOUT, self.receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(RequestFailure::SessionClosed),
            Err(_) => Err(RequestFailure::Timeout),
        }
    }
}

/// Handles a response that arrived after the caller abandoned the request.
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
                    "SUBSCRIBE_OK arrived after the request was abandoned; sending UNSUBSCRIBE"
                );
                session
                    .cancel_unclaimed_track_alias(connection, subscribe_ok.track_alias)
                    .await;
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
                    "SUBSCRIBE_NAMESPACE_OK arrived after the request was abandoned; sending UNSUBSCRIBE_NAMESPACE"
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
                    "PUBLISH_NAMESPACE_OK arrived after the request was abandoned; sending PUBLISH_NAMESPACE_DONE"
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
                    "PUBLISH_OK arrived after the request was abandoned; sending PUBLISH_DONE"
                );
                // Stream Count 0 (§9.12): `publish()` returns no
                // Subscription before PUBLISH_OK, so no stream was opened.
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
                    "FETCH_OK arrived after the request was abandoned; sending FETCH_CANCEL"
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
        "discarding response that arrived after the request was abandoned"
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
    use tokio::sync::mpsc::error::TryRecvError;

    use super::{LateResponseAction, handle_late_response};
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
                domains::connection::IncomingObjectNotification,
            },
            test_support::{
                HANDSHAKE_TIMEOUT, notify_datagram, register_and_take_data_receiver,
                spawn_connected_dual_sessions,
            },
        },
        object::{GroupOrder, Location},
        publish::PublishDoneCode,
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
            context.connection(),
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
            .register_data_receiver(context.connection(), OTHER_REQUEST_ID, TRACK_ALIAS)
            .await
            .unwrap();

        // Act
        handle_late_response(
            &context,
            context.connection(),
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
            context.connection(),
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
            context.connection(),
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
            context.connection(),
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
}

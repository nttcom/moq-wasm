//! The tasks that read draft-18 streams: the peer's control stream, the
//! request streams the peer opens, and the responses on the request
//! streams this endpoint opened. Draft-18 messages become the same session
//! events and responses draft-14 produces; the end of a request stream
//! becomes the cancellation message draft-14 would have sent.

use std::sync::{Arc, Weak};

use futures::future::{Either, select};
use tracing::{Instrument, Span};

use super::{
    ControlChannel, RequestStreamKind,
    message_stream::{MessageDecoder, MessageReceiver, MessageSender},
    messages::{self, Message, RequestErrorMessage, has_mandatory_track_property},
    parameters::Parameter,
};
use crate::{
    SessionEvent,
    modules::{
        executor::{self, JoinHandle},
        moqt::{
            control_plane::{
                constants::TerminationErrorCode,
                control_messages::messages::{
                    fetch_cancel::FetchCancel,
                    go_away::GoAway,
                    parameters::{
                        filter_type::FilterType, group_order::GroupOrder, location::Location,
                    },
                    publish_namespace_done::PublishNamespaceDone,
                    subscribe_update::SubscribeUpdate,
                    unsubscribe::Unsubscribe,
                    unsubscribe_namespace::UnsubscribeNamespace,
                },
                enums::{RequestId, ResponseMessage},
                error_codes::{PublishDoneCode, RequestErrorCode},
                handler::{
                    fetch_cancel_handler::FetchCancelHandler, fetch_handler::FetchHandler,
                    go_away_handler::GoAwayHandler, publish_done_handler::PublishDoneHandler,
                    publish_handler::PublishHandler,
                    publish_namespace_cancel_handler::PublishNamespaceCancelHandler,
                    publish_namespace_done_handler::PublishNamespaceDoneHandler,
                    publish_namespace_handler::PublishNamespaceHandler,
                    subscribe_handler::SubscribeHandler,
                    subscribe_namespace_handler::SubscribeNamespaceHandler,
                    subscribe_update_handler::SubscribeUpdateHandler,
                    track_status_handler::TrackStatusHandler,
                    unsubscribe_handler::UnsubscribeHandler,
                    unsubscribe_namespace_handler::UnsubscribeNamespaceHandler,
                },
            },
            domains::session_context::SessionContext,
        },
    },
};

fn emit(session: &SessionContext, event: SessionEvent) {
    if let Err(error) = session.event_sender.send(event) {
        tracing::debug!(?error, "session event dropped: the session is gone");
    }
}

fn close_on_violation(session: &SessionContext, reason: &str) {
    tracing::error!(
        reason,
        "Protocol violation on a draft-18 stream; closing session"
    );
    session.close_with_error(TerminationErrorCode::ProtocolViolation, reason);
}

/// Reads the peer's control stream and accepts the request streams the
/// peer opens, until the session ends.
pub(crate) fn run_control_receiver(
    control_stream: MessageReceiver,
    session: Weak<SessionContext>,
    span: Span,
) -> JoinHandle {
    executor::spawn(
        "draft-18 control receiver",
        async move {
            let control = Box::pin(read_control_stream(control_stream, session.clone()));
            let requests = Box::pin(accept_request_streams(session));
            match select(control, requests).await {
                Either::Left(_) | Either::Right(_) => {}
            }
        }
        .instrument(span),
    )
}

/// §3.3: after SETUP the control stream carries only GOAWAY, and it must
/// stay open for the whole session.
async fn read_control_stream(mut control_stream: MessageReceiver, session: Weak<SessionContext>) {
    loop {
        let received = control_stream.receive().await;
        let Some(session) = session.upgrade() else {
            return;
        };
        match received {
            Ok(Some(Message::GoAway(go_away))) => emit(
                &session,
                SessionEvent::GoAway(GoAwayHandler::new(GoAway::new(go_away.new_session_uri))),
            ),
            Ok(Some(_)) => {
                close_on_violation(&session, "unexpected message on the control stream");
                return;
            }
            Ok(None) => {
                close_on_violation(&session, "the control stream closed");
                return;
            }
            Err(error) => {
                tracing::debug!(%error, "control stream ended");
                return;
            }
        }
    }
}

// On wasm32 the browser transport is !Send, but the context is shared with
// the native build, where the tasks run on a multi-threaded executor.
#[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
async fn accept_request_streams(session: Weak<SessionContext>) {
    loop {
        let accepted = match session.upgrade() {
            Some(session) => session.transport_connection.accept_bi().await,
            None => return,
        };
        match accepted {
            Ok((send_stream, receive_stream)) => {
                IncomingRequestTask::run(
                    session.clone(),
                    Arc::new(MessageSender::new(send_stream)),
                    MessageReceiver::new(receive_stream, MessageDecoder),
                );
            }
            Err(error) => {
                tracing::debug!(%error, "stopped accepting request streams: the session ended");
                return;
            }
        }
    }
}

struct IncomingRequestTask;

impl IncomingRequestTask {
    fn run(session: Weak<SessionContext>, sender: Arc<MessageSender>, receiver: MessageReceiver) {
        executor::spawn(
            "draft-18 incoming request",
            Self::serve(session, sender, receiver).in_current_span(),
        );
    }

    async fn serve(
        session: Weak<SessionContext>,
        sender: Arc<MessageSender>,
        mut receiver: MessageReceiver,
    ) {
        let first = receiver.receive().await;
        let Some(context) = session.upgrade() else {
            return;
        };
        let message = match first {
            Ok(Some(message)) => message,
            Ok(None) => return,
            Err(error) => {
                close_on_violation(&context, &format!("malformed request stream: {error}"));
                return;
            }
        };
        let Some((request_id, kind)) = Self::accept_request(&context, &sender, message).await
        else {
            return;
        };
        drop(context);
        loop {
            let received = receiver.receive().await;
            let Some(context) = session.upgrade() else {
                return;
            };
            match received {
                Ok(Some(message)) => {
                    if !Self::follow_up(&context, &sender, request_id, &kind, message).await {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    Self::peer_ended(&context, request_id, kind);
                    return;
                }
            }
        }
    }

    /// Registers the peer's request and hands it to the application as the
    /// event draft-14 would raise. `None` when the stream is done.
    async fn accept_request(
        session: &Arc<SessionContext>,
        sender: &Arc<MessageSender>,
        message: Message,
    ) -> Option<(RequestId, RequestStreamKind)> {
        let channel = ControlChannel::of(session);
        let request_id = match &message {
            Message::Subscribe(m) | Message::TrackStatus(m) => m.request_id,
            Message::Fetch(m) => m.request_id,
            Message::Publish { publish, .. } => publish.request_id,
            Message::PublishNamespace(m) => m.request_id,
            Message::SubscribeNamespace(m) => m.request_id,
            Message::SubscribeTracks(m) => m.request_id,
            _ => {
                close_on_violation(session, "a request stream does not begin with a request");
                return None;
            }
        };
        if !channel.accept_peer_request_id(request_id) {
            tracing::error!(
                request_id,
                "invalid or repeated Request ID; closing session"
            );
            session.close_with_error(TerminationErrorCode::InvalidRequestId, "invalid request id");
            return None;
        }
        let (kind, event) = match message {
            Message::Subscribe(subscribe) => (
                RequestStreamKind::Subscribe,
                SessionEvent::Subscribe(SubscribeHandler::new(session.clone(), subscribe)),
            ),
            Message::TrackStatus(track_status) => (
                RequestStreamKind::TrackStatus,
                SessionEvent::TrackStatus(TrackStatusHandler::new(session.clone(), track_status)),
            ),
            Message::Fetch(fetch) => (
                RequestStreamKind::Fetch,
                SessionEvent::Fetch(FetchHandler::new(session.clone(), fetch)),
            ),
            Message::Publish {
                publish,
                track_properties,
            } => {
                if has_mandatory_track_property(&track_properties) {
                    Self::reject(sender, RequestErrorCode::UnsupportedExtension).await;
                    return None;
                }
                (
                    RequestStreamKind::Publish,
                    SessionEvent::Publish(PublishHandler::new(session.clone(), publish)),
                )
            }
            Message::PublishNamespace(publish_namespace) => (
                RequestStreamKind::PublishNamespace(publish_namespace.track_namespace.clone()),
                SessionEvent::PublishNamespace(PublishNamespaceHandler::new(
                    session.clone(),
                    publish_namespace,
                )),
            ),
            Message::SubscribeNamespace(subscribe_namespace) => (
                RequestStreamKind::SubscribeNamespace(
                    subscribe_namespace.track_namespace_prefix.clone(),
                ),
                SessionEvent::SubscribeNameSpace(SubscribeNamespaceHandler::new(
                    session.clone(),
                    subscribe_namespace,
                )),
            ),
            Message::SubscribeTracks(_) => {
                Self::reject(sender, RequestErrorCode::NotSupported).await;
                return None;
            }
            _ => unreachable!("non-request messages returned above"),
        };
        // Registered before the event, so a handler can answer at once.
        channel.register_incoming(request_id, kind.clone(), sender.clone());
        emit(session, event);
        Some((request_id, kind))
    }

    async fn reject(sender: &MessageSender, code: RequestErrorCode) {
        let Ok(message) = messages::encode_request_error(code, "") else {
            return;
        };
        if sender.send(&message).await.is_ok() {
            let _ = sender.close().await;
        }
    }

    /// `false` when the stream is done.
    async fn follow_up(
        session: &Arc<SessionContext>,
        sender: &MessageSender,
        request_id: RequestId,
        kind: &RequestStreamKind,
        message: Message,
    ) -> bool {
        match (kind, message) {
            (RequestStreamKind::Subscribe, Message::RequestUpdate(update)) => {
                let update = subscribe_update(request_id, update.request_id, update.parameters);
                emit(
                    session,
                    SessionEvent::SubscribeUpdate(SubscribeUpdateHandler::new(update)),
                );
                acknowledge_update(sender).await;
                true
            }
            (_, Message::RequestUpdate(_)) => {
                acknowledge_update(sender).await;
                true
            }
            (RequestStreamKind::Publish, Message::PublishDone(done)) => {
                emit(
                    session,
                    SessionEvent::PublishDone(PublishDoneHandler::new(
                        request_id,
                        done.code,
                        done.stream_count,
                        done.reason,
                    )),
                );
                ControlChannel::of(session).take_incoming(request_id);
                false
            }
            (_, Message::GoAway(_)) => {
                tracing::debug!(request_id, "per-request GOAWAY ignored");
                true
            }
            _ => {
                close_on_violation(session, "unexpected message on a request stream");
                false
            }
        }
    }

    /// The peer closed or reset its half: for an open request, that
    /// withdraws it (§3.3.2), as draft-14's UNSUBSCRIBE / FETCH_CANCEL /
    /// PUBLISH_NAMESPACE_DONE / UNSUBSCRIBE_NAMESPACE would.
    fn peer_ended(session: &Arc<SessionContext>, request_id: RequestId, kind: RequestStreamKind) {
        if ControlChannel::of(session)
            .take_incoming(request_id)
            .is_none()
        {
            return;
        }
        let event = match kind {
            RequestStreamKind::Subscribe => SessionEvent::Unsubscribe(UnsubscribeHandler::new(
                session.clone(),
                Unsubscribe { request_id },
            )),
            RequestStreamKind::Fetch => {
                SessionEvent::FetchCancel(FetchCancelHandler::new(FetchCancel::new(request_id)))
            }
            RequestStreamKind::PublishNamespace(namespace) => SessionEvent::PublishNamespaceDone(
                PublishNamespaceDoneHandler::new(PublishNamespaceDone::new(namespace)),
            ),
            RequestStreamKind::SubscribeNamespace(prefix) => SessionEvent::UnsubscribeNamespace(
                UnsubscribeNamespaceHandler::new(UnsubscribeNamespace::new(prefix)),
            ),
            RequestStreamKind::Publish => SessionEvent::PublishDone(PublishDoneHandler::new(
                request_id,
                PublishDoneCode::InternalError,
                0,
                "publish request stream ended".to_string(),
            )),
            RequestStreamKind::TrackStatus => return,
        };
        emit(session, event);
    }
}

async fn acknowledge_update(sender: &MessageSender) {
    if let Ok(message) = messages::encode_empty_request_ok()
        && let Err(error) = sender.send(&message).await
    {
        tracing::debug!(%error, "could not acknowledge REQUEST_UPDATE");
    }
}

/// draft-18 updates carry only what changes; the model's update states the
/// whole subscription, so absent fields take their defaults.
fn subscribe_update(
    subscription_request_id: RequestId,
    request_id: RequestId,
    parameters: Vec<Parameter>,
) -> SubscribeUpdate {
    let mut update = SubscribeUpdate {
        request_id,
        subscription_request_id,
        start_location: Location {
            group_id: 0,
            object_id: 0,
        },
        end_group: 0,
        subscriber_priority: 128,
        forward: true,
        authorization_tokens: vec![],
        delivery_timeout: None,
    };
    for parameter in parameters {
        match parameter {
            Parameter::SubscriberPriority(priority) => update.subscriber_priority = priority,
            Parameter::Forward(forward) => update.forward = forward,
            Parameter::AuthorizationToken(token) => update.authorization_tokens.push(token),
            Parameter::ObjectDeliveryTimeout(timeout) => update.delivery_timeout = Some(timeout),
            Parameter::SubscriptionFilter(FilterType::AbsoluteStart { location }) => {
                update.start_location = location
            }
            Parameter::SubscriptionFilter(FilterType::AbsoluteRange {
                location,
                end_group,
            }) => {
                update.start_location = location;
                update.end_group = end_group + 1;
            }
            _ => {}
        }
    }
    update
}

/// Reads the responses on a request stream this endpoint opened.
pub(crate) struct OutgoingRequestTask;

impl OutgoingRequestTask {
    pub(crate) fn run(
        session: Weak<SessionContext>,
        request_id: RequestId,
        kind: RequestStreamKind,
        receiver: MessageReceiver,
        response: tokio::sync::oneshot::Sender<ResponseMessage>,
        group_order: GroupOrder,
    ) {
        executor::spawn(
            "draft-18 outgoing request",
            Self::read(session, request_id, kind, receiver, response, group_order)
                .in_current_span(),
        );
    }

    async fn read(
        session: Weak<SessionContext>,
        request_id: RequestId,
        kind: RequestStreamKind,
        mut receiver: MessageReceiver,
        response: tokio::sync::oneshot::Sender<ResponseMessage>,
        group_order: GroupOrder,
    ) {
        let first = receiver.receive().await;
        let Some(context) = session.upgrade() else {
            return;
        };
        let message = match first {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => {
                ControlChannel::of(&context).take_outgoing(request_id);
                return;
            }
        };
        let Some(first_response) = Self::first_response(request_id, &kind, message, group_order)
        else {
            close_on_violation(&context, "the first response does not match the request");
            return;
        };
        let succeeded = !matches!(
            first_response,
            ResponseMessage::SubscribeError(..)
                | ResponseMessage::PublishError(..)
                | ResponseMessage::FetchError(..)
                | ResponseMessage::TrackStatusError(..)
                | ResponseMessage::PublishNamespaceError(..)
                | ResponseMessage::SubscribeNameSpaceError(..)
        );
        if response.send(first_response).is_err() || !succeeded {
            return;
        }
        drop(context);
        let mut publish_done = false;
        loop {
            let received = receiver.receive().await;
            let Some(context) = session.upgrade() else {
                return;
            };
            match received {
                Ok(Some(Message::PublishDone(done))) if kind == RequestStreamKind::Subscribe => {
                    publish_done = true;
                    emit(
                        &context,
                        SessionEvent::PublishDone(PublishDoneHandler::new(
                            request_id,
                            done.code,
                            done.stream_count,
                            done.reason,
                        )),
                    );
                }
                Ok(Some(Message::RequestUpdate(update))) if kind == RequestStreamKind::Publish => {
                    let update = subscribe_update(request_id, update.request_id, update.parameters);
                    emit(
                        &context,
                        SessionEvent::SubscribeUpdate(SubscribeUpdateHandler::new(update)),
                    );
                    if let Some(stream) = outgoing_sender(&context, request_id) {
                        acknowledge_update(&stream).await;
                    }
                }
                // Answers to our REQUEST_UPDATE.
                Ok(Some(Message::RequestOk(_) | Message::RequestError(_))) => {}
                Ok(Some(Message::Namespace(_) | Message::NamespaceDone(_)))
                    if matches!(kind, RequestStreamKind::SubscribeNamespace(_)) =>
                {
                    tracing::debug!(request_id, "NAMESPACE / NAMESPACE_DONE are not mapped yet");
                }
                Ok(Some(Message::GoAway(_))) => {
                    tracing::debug!(request_id, "per-request GOAWAY ignored");
                }
                Ok(Some(_)) => {
                    close_on_violation(&context, "unexpected message on a request stream");
                    return;
                }
                Ok(None) | Err(_) => {
                    Self::peer_ended(&context, request_id, kind, publish_done);
                    return;
                }
            }
        }
    }

    fn rejected(
        request_id: RequestId,
        error: RequestErrorMessage,
        into: fn(RequestId, RequestErrorCode, String) -> ResponseMessage,
    ) -> ResponseMessage {
        into(request_id, error.code, error.reason)
    }

    fn first_response(
        request_id: RequestId,
        kind: &RequestStreamKind,
        message: Message,
        group_order: GroupOrder,
    ) -> Option<ResponseMessage> {
        Some(match (kind, message) {
            (RequestStreamKind::Subscribe, Message::SubscribeOk(ok)) => {
                if has_mandatory_track_property(&ok.track_properties) {
                    ResponseMessage::SubscribeError(
                        request_id,
                        RequestErrorCode::UnsupportedExtension,
                        "mandatory track property not understood".to_string(),
                    )
                } else {
                    ResponseMessage::SubscribeOk(ok.into_subscribe_ok(request_id))
                }
            }
            (RequestStreamKind::Subscribe, Message::RequestError(error)) => {
                Self::rejected(request_id, error, ResponseMessage::SubscribeError)
            }
            (RequestStreamKind::TrackStatus, Message::RequestOk(ok)) => {
                ResponseMessage::TrackStatusOk(ok.into_track_status_ok(request_id))
            }
            (RequestStreamKind::TrackStatus, Message::RequestError(error)) => {
                Self::rejected(request_id, error, ResponseMessage::TrackStatusError)
            }
            (RequestStreamKind::Fetch, Message::FetchOk(ok)) => {
                if has_mandatory_track_property(&ok.track_properties) {
                    ResponseMessage::FetchError(
                        request_id,
                        RequestErrorCode::UnsupportedExtension,
                        "mandatory track property not understood".to_string(),
                    )
                } else {
                    ResponseMessage::FetchOk(ok.into_fetch_ok(request_id, group_order))
                }
            }
            (RequestStreamKind::Fetch, Message::RequestError(error)) => {
                Self::rejected(request_id, error, ResponseMessage::FetchError)
            }
            (RequestStreamKind::Publish, Message::RequestOk(ok)) => {
                ResponseMessage::PublishOk(ok.into_publish_ok(request_id))
            }
            (RequestStreamKind::Publish, Message::RequestError(error)) => {
                Self::rejected(request_id, error, ResponseMessage::PublishError)
            }
            (RequestStreamKind::PublishNamespace(_), Message::RequestOk(_)) => {
                ResponseMessage::PublishNamespaceOk(request_id)
            }
            (RequestStreamKind::PublishNamespace(_), Message::RequestError(error)) => {
                Self::rejected(request_id, error, ResponseMessage::PublishNamespaceError)
            }
            (RequestStreamKind::SubscribeNamespace(_), Message::RequestOk(_)) => {
                ResponseMessage::SubscribeNameSpaceOk(request_id)
            }
            (RequestStreamKind::SubscribeNamespace(_), Message::RequestError(error)) => {
                Self::rejected(request_id, error, ResponseMessage::SubscribeNameSpaceError)
            }
            _ => return None,
        })
    }

    /// The peer ended a request this endpoint still had open: for a
    /// subscription without PUBLISH_DONE, a publication or a namespace, that
    /// is the withdrawal draft-14 would have sent as a message.
    fn peer_ended(
        session: &Arc<SessionContext>,
        request_id: RequestId,
        kind: RequestStreamKind,
        publish_done: bool,
    ) {
        if ControlChannel::of(session)
            .take_outgoing(request_id)
            .is_none()
        {
            return;
        }
        let event = match kind {
            RequestStreamKind::Subscribe if !publish_done => {
                SessionEvent::PublishDone(PublishDoneHandler::new(
                    request_id,
                    PublishDoneCode::InternalError,
                    0,
                    "subscription stream ended".to_string(),
                ))
            }
            RequestStreamKind::Publish => SessionEvent::Unsubscribe(UnsubscribeHandler::new(
                session.clone(),
                Unsubscribe { request_id },
            )),
            RequestStreamKind::PublishNamespace(namespace) => {
                SessionEvent::PublishNamespaceCancel(PublishNamespaceCancelHandler::new(
                    namespace.join("/"),
                    RequestErrorCode::InternalError,
                    "publish namespace stream ended".to_string(),
                ))
            }
            _ => return,
        };
        emit(session, event);
    }
}

fn outgoing_sender(session: &SessionContext, request_id: RequestId) -> Option<Arc<MessageSender>> {
    ControlChannel::of(session)
        .requests()
        .outgoing
        .get(&request_id)
        .map(|stream| stream.sender.clone())
}

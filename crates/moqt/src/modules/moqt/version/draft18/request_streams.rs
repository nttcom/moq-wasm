//! The tasks that read draft-18 streams: the peer's control stream, the
//! request streams the peer opens, and the responses on the request
//! streams this endpoint opened. Draft-18 messages become the same session
//! events and responses draft-14 produces; the end of a request stream
//! becomes the cancellation message draft-14 would have sent.

use std::{
    collections::HashSet,
    sync::{Arc, Weak},
};

use futures::future::select;
use tracing::{Instrument, Span};

use super::{
    ControlChannel, RequestStreamKind, Scope,
    message_stream::{MessageReceiveError, MessageReceiver, MessageSender},
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
                    publish_namespace::PublishNamespace,
                    publish_namespace_done::PublishNamespaceDone,
                    subscribe_namespace::SubscribeNamespace,
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
            domains::{connection::Connection, session_context::SessionContext},
        },
    },
};

fn emit(scope: &Scope, event: SessionEvent) {
    if let SessionEvent::PublishDone(handler) = &event
        && let Some(publish_done_sender) = scope.channel().publish_done_sender()
    {
        let _ = publish_done_sender.send(handler.request_id());
    }
    if let Err(error) = scope.connection.event_sender.send(event) {
        tracing::debug!(?error, "session event dropped: the session is gone");
    }
}

fn close_on_violation(scope: &Scope, reason: &str) {
    tracing::error!(
        reason,
        "Protocol violation on a draft-18 stream; closing session"
    );
    scope.session.close_connection(
        &scope.connection,
        TerminationErrorCode::ProtocolViolation,
        reason,
    );
}

/// Reads the peer's control stream and accepts the request streams the
/// peer opens, until the connection ends. The Request ID of every
/// PUBLISH_DONE also goes to `publish_done_sender`.
pub(crate) fn run_control_receiver(
    control_stream: MessageReceiver,
    session: Weak<SessionContext>,
    connection: Weak<Connection>,
    publish_done_sender: tokio::sync::mpsc::UnboundedSender<RequestId>,
    span: Span,
) -> JoinHandle {
    if let Some(connection) = connection.upgrade() {
        ControlChannel::of(&connection).bind(session, publish_done_sender);
    }
    executor::spawn(
        "draft-18 control receiver",
        async move {
            let control = Box::pin(read_control_stream(control_stream, connection.clone()));
            let requests = Box::pin(accept_request_streams(connection));
            select(control, requests).await;
        }
        .instrument(span),
    )
}

/// §3.3: after SETUP the control stream carries only GOAWAY, and it must
/// stay open for the whole session.
async fn read_control_stream(mut control_stream: MessageReceiver, connection: Weak<Connection>) {
    loop {
        let received = control_stream.receive().await;
        let Some(scope) = Scope::of(&connection) else {
            return;
        };
        match received {
            Ok(Some(Message::GoAway(go_away))) => {
                if let Err(reason) = scope
                    .connection
                    .note_go_away_received(&go_away.new_session_uri)
                {
                    close_on_violation(&scope, reason);
                    return;
                }
                emit(
                    &scope,
                    SessionEvent::GoAway(GoAwayHandler::new(GoAway::new(go_away.new_session_uri))),
                );
            }
            Ok(Some(_)) => {
                close_on_violation(&scope, "unexpected message on the control stream");
                return;
            }
            Ok(None) | Err(MessageReceiveError::Reset) => {
                close_on_violation(&scope, "the control stream closed");
                return;
            }
            Err(MessageReceiveError::Malformed(reason)) => {
                close_on_violation(&scope, &reason);
                return;
            }
            Err(MessageReceiveError::SessionEnded) => {
                tracing::debug!("control stream ended with the session");
                return;
            }
        }
    }
}

// On wasm32 the browser transport is !Send, but the context is shared with
// the native build, where the tasks run on a multi-threaded executor.
#[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
async fn accept_request_streams(connection: Weak<Connection>) {
    loop {
        let accepted = match connection.upgrade() {
            Some(connection) => connection.transport.accept_bi().await,
            None => return,
        };
        match accepted {
            Ok((send_stream, receive_stream)) => {
                IncomingRequestTask::run(
                    connection.clone(),
                    Arc::new(MessageSender::new(send_stream)),
                    MessageReceiver::new(receive_stream),
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
    fn run(connection: Weak<Connection>, sender: Arc<MessageSender>, receiver: MessageReceiver) {
        executor::spawn(
            "draft-18 incoming request",
            Self::serve(connection, sender, receiver).in_current_span(),
        );
    }

    async fn serve(
        connection: Weak<Connection>,
        sender: Arc<MessageSender>,
        mut receiver: MessageReceiver,
    ) {
        let first = receiver.receive().await;
        let Some(context) = Scope::of(&connection) else {
            return;
        };
        let message = match first {
            Ok(Some(message)) => message,
            Err(MessageReceiveError::Malformed(reason)) => {
                close_on_violation(&context, &reason);
                return;
            }
            Ok(None) | Err(MessageReceiveError::Reset | MessageReceiveError::SessionEnded) => {
                return;
            }
        };
        let Some((request_id, kind, mut baseline)) =
            Self::accept_request(&context, &sender, message).await
        else {
            return;
        };
        drop(context);
        loop {
            let received = receiver.receive().await;
            let Some(context) = Scope::of(&connection) else {
                return;
            };
            match received {
                Ok(Some(message)) => {
                    if !Self::follow_up(
                        &context,
                        &sender,
                        request_id,
                        &kind,
                        &mut baseline,
                        message,
                    )
                    .await
                    {
                        return;
                    }
                }
                Ok(None) | Err(MessageReceiveError::Reset) => {
                    Self::peer_ended(&context, request_id, kind);
                    return;
                }
                Err(MessageReceiveError::Malformed(reason)) => {
                    close_on_violation(&context, &reason);
                    return;
                }
                Err(MessageReceiveError::SessionEnded) => return,
            }
        }
    }

    /// Registers the peer's request and hands it to the application as the
    /// event draft-14 would raise. `None` when the stream is done.
    async fn accept_request(
        scope: &Scope,
        sender: &Arc<MessageSender>,
        message: Message,
    ) -> Option<(RequestId, RequestStreamKind, Option<SubscribeUpdate>)> {
        let channel = scope.channel();
        let request_id = match &message {
            Message::Subscribe(m) | Message::TrackStatus(m) => m.request_id,
            Message::Fetch(m) => m.request_id,
            Message::Publish { publish, .. } => publish.request_id,
            Message::PublishNamespace(m) => m.request_id,
            Message::SubscribeNamespace(m) => m.request_id,
            Message::SubscribeTracks(m) => m.request_id,
            _ => {
                close_on_violation(scope, "a request stream does not begin with a request");
                return None;
            }
        };
        if !channel.accept_peer_request_id(request_id) {
            tracing::error!(
                request_id,
                "invalid or repeated Request ID; closing session"
            );
            scope.session.close_connection(
                &scope.connection,
                TerminationErrorCode::InvalidRequestId,
                "invalid request id",
            );
            return None;
        }
        let mut baseline = None;
        let (kind, event) = match message {
            Message::Subscribe(subscribe) => {
                baseline = Some(update_baseline(
                    subscribe.request_id,
                    subscribe.subscriber_priority,
                    subscribe.forward,
                    subscribe.filter_type,
                    subscribe.delivery_timeout,
                ));
                (
                    RequestStreamKind::Subscribe,
                    SessionEvent::Subscribe(SubscribeHandler::new(
                        scope.session.clone(),
                        scope.connection.clone(),
                        subscribe,
                    )),
                )
            }
            Message::TrackStatus(track_status) => (
                RequestStreamKind::TrackStatus,
                SessionEvent::TrackStatus(TrackStatusHandler::new(
                    scope.connection.clone(),
                    track_status,
                )),
            ),
            Message::Fetch(fetch) => (
                RequestStreamKind::Fetch,
                SessionEvent::Fetch(FetchHandler::new(
                    scope.session.clone(),
                    scope.connection.clone(),
                    fetch,
                )),
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
                    SessionEvent::Publish(PublishHandler::new(
                        scope.session.clone(),
                        scope.connection.clone(),
                        publish,
                    )),
                )
            }
            Message::PublishNamespace(publish_namespace) => (
                RequestStreamKind::PublishNamespace(publish_namespace.track_namespace.clone()),
                SessionEvent::PublishNamespace(PublishNamespaceHandler::new(
                    scope.connection.clone(),
                    publish_namespace,
                )),
            ),
            Message::SubscribeNamespace(subscribe_namespace) => (
                RequestStreamKind::SubscribeNamespace(
                    subscribe_namespace.track_namespace_prefix.clone(),
                ),
                SessionEvent::SubscribeNameSpace(SubscribeNamespaceHandler::new(
                    scope.connection.clone(),
                    subscribe_namespace,
                )),
            ),
            Message::SubscribeTracks(subscribe_tracks) => {
                let tokens = subscribe_tracks
                    .parameters
                    .into_iter()
                    .filter_map(|parameter| match parameter {
                        Parameter::AuthorizationToken(token) => Some(token),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                (
                    RequestStreamKind::SubscribeTracks(
                        subscribe_tracks.track_namespace_prefix.clone(),
                    ),
                    SessionEvent::SubscribeNameSpace(SubscribeNamespaceHandler::new(
                        scope.connection.clone(),
                        SubscribeNamespace {
                            request_id: subscribe_tracks.request_id,
                            track_namespace_prefix: subscribe_tracks.track_namespace_prefix,
                            authorization_token: tokens,
                        },
                    )),
                )
            }
            _ => unreachable!("non-request messages returned above"),
        };
        // Registered before the event, so a handler can answer at once.
        channel.register_incoming(request_id, kind.clone(), sender.clone());
        emit(scope, event);
        Some((request_id, kind, baseline))
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
        scope: &Scope,
        sender: &MessageSender,
        request_id: RequestId,
        kind: &RequestStreamKind,
        baseline: &mut Option<SubscribeUpdate>,
        message: Message,
    ) -> bool {
        match (kind, message) {
            (RequestStreamKind::Subscribe, Message::RequestUpdate(update)) => {
                let Some(baseline) = baseline else {
                    unreachable!("a SUBSCRIBE stream keeps its baseline")
                };
                let update = apply_update(baseline, update.request_id, update.parameters);
                emit(
                    scope,
                    SessionEvent::SubscribeUpdate(SubscribeUpdateHandler::new(
                        &scope.connection,
                        update,
                    )),
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
                    scope,
                    SessionEvent::PublishDone(PublishDoneHandler::new(
                        request_id,
                        done.code,
                        done.stream_count,
                        done.reason,
                    )),
                );
                scope.channel().take_incoming(request_id);
                false
            }
            (_, Message::GoAway(_)) => {
                tracing::debug!(request_id, "per-request GOAWAY ignored");
                true
            }
            _ => {
                close_on_violation(scope, "unexpected message on a request stream");
                false
            }
        }
    }

    /// The peer closed or reset its half: for an open request, that
    /// withdraws it (§3.3.2), as draft-14's UNSUBSCRIBE / FETCH_CANCEL /
    /// PUBLISH_NAMESPACE_DONE / UNSUBSCRIBE_NAMESPACE would.
    fn peer_ended(scope: &Scope, request_id: RequestId, kind: RequestStreamKind) {
        if scope.channel().take_incoming(request_id).is_none() {
            return;
        }
        let event = match kind {
            RequestStreamKind::Subscribe => SessionEvent::Unsubscribe(UnsubscribeHandler::new(
                scope.session.clone(),
                &scope.connection,
                Unsubscribe { request_id },
            )),
            RequestStreamKind::Fetch => SessionEvent::FetchCancel(FetchCancelHandler::new(
                scope.session.clone(),
                &scope.connection,
                FetchCancel::new(request_id),
            )),
            RequestStreamKind::PublishNamespace(namespace) => SessionEvent::PublishNamespaceDone(
                PublishNamespaceDoneHandler::new(PublishNamespaceDone::new(namespace)),
            ),
            RequestStreamKind::SubscribeNamespace(prefix)
            | RequestStreamKind::SubscribeTracks(prefix) => {
                scope.channel().forget_advertised(request_id);
                SessionEvent::UnsubscribeNamespace(UnsubscribeNamespaceHandler::new(
                    UnsubscribeNamespace::new(prefix),
                ))
            }
            RequestStreamKind::Publish => SessionEvent::PublishDone(PublishDoneHandler::new(
                request_id,
                PublishDoneCode::InternalError,
                0,
                "publish request stream ended".to_string(),
            )),
            RequestStreamKind::TrackStatus => return,
        };
        emit(scope, event);
    }
}

async fn acknowledge_update(sender: &MessageSender) {
    if let Ok(message) = messages::encode_empty_request_ok()
        && let Err(error) = sender.send(&message).await
    {
        tracing::debug!(%error, "could not acknowledge REQUEST_UPDATE");
    }
}

/// The subscription state a REQUEST_UPDATE applies to, in draft-14's
/// SUBSCRIBE_UPDATE form: the end group is the last group plus 1, 0 for none.
/// A relative filter has no absolute start, so it starts at {0, 0}.
fn update_baseline(
    subscription_request_id: RequestId,
    subscriber_priority: u8,
    forward: bool,
    filter: FilterType,
    delivery_timeout: Option<u64>,
) -> SubscribeUpdate {
    let (start_location, end_group) = filter_range(filter);
    SubscribeUpdate {
        request_id: subscription_request_id,
        subscription_request_id,
        start_location,
        end_group,
        subscriber_priority,
        forward,
        authorization_tokens: vec![],
        delivery_timeout,
    }
}

fn filter_range(filter: FilterType) -> (Location, u64) {
    match filter {
        FilterType::AbsoluteStart { location } => (location, 0),
        FilterType::AbsoluteRange {
            location,
            end_group,
        } => (location, end_group + 1),
        FilterType::LargestObject | FilterType::NextGroupStart => (
            Location {
                group_id: 0,
                object_id: 0,
            },
            0,
        ),
    }
}

/// §10.9: a REQUEST_UPDATE carries only the parameters that change, while
/// draft-14's SUBSCRIBE_UPDATE carries the whole state, so the update is
/// applied to `baseline`.
fn apply_update(
    baseline: &mut SubscribeUpdate,
    request_id: RequestId,
    parameters: Vec<Parameter>,
) -> SubscribeUpdate {
    baseline.request_id = request_id;
    baseline.authorization_tokens.clear();
    for parameter in parameters {
        match parameter {
            Parameter::SubscriberPriority(priority) => baseline.subscriber_priority = priority,
            Parameter::Forward(forward) => baseline.forward = forward,
            Parameter::AuthorizationToken(token) => baseline.authorization_tokens.push(token),
            Parameter::ObjectDeliveryTimeout(timeout) => baseline.delivery_timeout = Some(timeout),
            Parameter::SubscriptionFilter(filter) => {
                (baseline.start_location, baseline.end_group) = filter_range(filter)
            }
            _ => {}
        }
    }
    baseline.clone()
}

pub(crate) struct OutgoingRequestTask;

impl OutgoingRequestTask {
    pub(crate) fn run(
        connection: Weak<Connection>,
        request_id: RequestId,
        kind: RequestStreamKind,
        receiver: MessageReceiver,
        response: tokio::sync::oneshot::Sender<ResponseMessage>,
        group_order: GroupOrder,
    ) {
        executor::spawn(
            "draft-18 outgoing request",
            Self::read(
                connection,
                request_id,
                kind,
                receiver,
                response,
                group_order,
            )
            .in_current_span(),
        );
    }

    async fn read(
        connection: Weak<Connection>,
        request_id: RequestId,
        kind: RequestStreamKind,
        mut receiver: MessageReceiver,
        response: tokio::sync::oneshot::Sender<ResponseMessage>,
        group_order: GroupOrder,
    ) {
        let first = receiver.receive().await;
        let Some(context) = Scope::of(&connection) else {
            return;
        };
        let message = match first {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => {
                context.channel().take_outgoing(request_id);
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
        // The subscriber's PUBLISH_OK is where its updates start from.
        let mut baseline = match &first_response {
            ResponseMessage::PublishOk(ok) => Some(update_baseline(
                request_id,
                ok.subscriber_priority,
                ok.forward,
                ok.filter_type,
                ok.delivery_timeout,
            )),
            _ => None,
        };
        if response.send(first_response).is_err() || !succeeded {
            return;
        }
        drop(context);
        let mut publish_done = false;
        // Namespaces a SUBSCRIBE_NAMESPACE stream announced and not withdrawn.
        let mut namespaces: HashSet<Vec<String>> = HashSet::new();
        loop {
            let received = receiver.receive().await;
            let Some(context) = Scope::of(&connection) else {
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
                    let Some(baseline) = &mut baseline else {
                        unreachable!("an accepted PUBLISH keeps its baseline")
                    };
                    let update = apply_update(baseline, update.request_id, update.parameters);
                    emit(
                        &context,
                        SessionEvent::SubscribeUpdate(SubscribeUpdateHandler::new(
                            &context.connection,
                            update,
                        )),
                    );
                    if let Some(stream) = outgoing_sender(&context, request_id) {
                        acknowledge_update(&stream).await;
                    }
                }
                // Answers to our REQUEST_UPDATE.
                Ok(Some(Message::RequestOk(_) | Message::RequestError(_))) => {}
                Ok(Some(Message::Namespace(suffix))) => {
                    let RequestStreamKind::SubscribeNamespace(prefix) = &kind else {
                        close_on_violation(&context, "NAMESPACE outside SUBSCRIBE_NAMESPACE");
                        return;
                    };
                    let namespace = [prefix.as_slice(), suffix.as_slice()].concat();
                    if namespaces.insert(namespace.clone()) {
                        Self::namespace_announced(&context, namespace);
                    }
                }
                Ok(Some(Message::NamespaceDone(suffix))) => {
                    let RequestStreamKind::SubscribeNamespace(prefix) = &kind else {
                        close_on_violation(&context, "NAMESPACE_DONE outside SUBSCRIBE_NAMESPACE");
                        return;
                    };
                    let namespace = [prefix.as_slice(), suffix.as_slice()].concat();
                    // §10.18: NAMESPACE_DONE must follow its NAMESPACE.
                    if !namespaces.remove(&namespace) {
                        close_on_violation(&context, "NAMESPACE_DONE before its NAMESPACE");
                        return;
                    }
                    Self::namespace_withdrawn(&context, namespace);
                }
                Ok(Some(Message::GoAway(go_away))) => {
                    tracing::info!(
                        request_id,
                        new_session_uri = go_away.new_session_uri,
                        "GOAWAY on a request stream; ending the request"
                    );
                    Self::moved_away(&context, request_id, kind, publish_done).await;
                    return;
                }
                Ok(Some(_)) => {
                    close_on_violation(&context, "unexpected message on a request stream");
                    return;
                }
                Err(MessageReceiveError::Malformed(reason)) => {
                    close_on_violation(&context, &reason);
                    return;
                }
                Err(MessageReceiveError::SessionEnded) => return,
                Ok(None) | Err(MessageReceiveError::Reset) => {
                    // §10.18: the end of the stream withdraws every namespace.
                    for namespace in namespaces {
                        Self::namespace_withdrawn(&context, namespace);
                    }
                    Self::peer_ended(&context, request_id, kind, publish_done);
                    return;
                }
            }
        }
    }

    /// A NAMESPACE reaches the application as draft-14's PUBLISH_NAMESPACE,
    /// under a Request ID made up for it: NAMESPACE takes no response, so
    /// the handler's answer goes nowhere.
    fn namespace_announced(scope: &Scope, namespace: Vec<String>) {
        let request_id = scope.channel().synthetic_request_id();
        emit(
            scope,
            SessionEvent::PublishNamespace(PublishNamespaceHandler::new(
                scope.connection.clone(),
                PublishNamespace {
                    request_id,
                    track_namespace: namespace,
                    authorization_token: vec![],
                },
            )),
        );
    }

    fn namespace_withdrawn(scope: &Scope, namespace: Vec<String>) {
        emit(
            scope,
            SessionEvent::PublishNamespaceDone(PublishNamespaceDoneHandler::new(
                PublishNamespaceDone::new(namespace),
            )),
        );
    }

    /// §10.4: a GOAWAY on a request stream moves that request alone.
    /// draft-14 events have no per-request migration, so the request ends
    /// as if the peer had ended it with GOING_AWAY, and its stream is reset.
    async fn moved_away(
        scope: &Scope,
        request_id: RequestId,
        kind: RequestStreamKind,
        publish_done: bool,
    ) {
        let Some(stream) = scope.channel().take_outgoing(request_id) else {
            return;
        };
        let event = match kind {
            RequestStreamKind::Subscribe if !publish_done => {
                Some(SessionEvent::PublishDone(PublishDoneHandler::new(
                    request_id,
                    PublishDoneCode::GoingAway,
                    0,
                    "request moved by GOAWAY".to_string(),
                )))
            }
            RequestStreamKind::Publish => Some(SessionEvent::Unsubscribe(UnsubscribeHandler::new(
                scope.session.clone(),
                &scope.connection,
                Unsubscribe { request_id },
            ))),
            RequestStreamKind::PublishNamespace(namespace) => Some(
                SessionEvent::PublishNamespaceCancel(PublishNamespaceCancelHandler::new(
                    namespace.join("/"),
                    RequestErrorCode::GoingAway,
                    "request moved by GOAWAY".to_string(),
                )),
            ),
            _ => None,
        };
        if let Some(event) = event {
            emit(scope, event);
        }
        let going_away = super::error_codes::stream_reset_code_to_wire(
            crate::modules::moqt::control_plane::error_codes::DataStreamResetCode::GoingAway,
        );
        if let Err(error) = stream.sender.reset(going_away).await {
            tracing::debug!(%error, "could not cancel a request moved by GOAWAY");
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
        scope: &Scope,
        request_id: RequestId,
        kind: RequestStreamKind,
        publish_done: bool,
    ) {
        if scope.channel().take_outgoing(request_id).is_none() {
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
                scope.session.clone(),
                &scope.connection,
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
        emit(scope, event);
    }
}

fn outgoing_sender(scope: &Scope, request_id: RequestId) -> Option<Arc<MessageSender>> {
    scope
        .channel()
        .requests()
        .outgoing
        .get(&request_id)
        .map(|stream| stream.sender.clone())
}

//! The tasks that read draft-18 streams: the peer's control stream, the
//! request streams the peer opens, and the responses on the request
//! streams this endpoint opened. Draft-18 messages become the same session
//! events and responses draft-14 produces; the end of a request stream
//! becomes the cancellation message draft-14 would have sent.

use std::{
    collections::HashSet,
    sync::{Arc, Weak},
};

use futures::{StreamExt, future::select, stream::FuturesOrdered};
use tracing::{Instrument, Span};

use super::{
    ControlChannel, RequestStream, RequestStreamKind, Scope, is_success,
    message_stream::{MessageDecoder, MessageReceiver},
    messages::{self, Message, has_mandatory_track_property},
    parameters::Parameter,
};
use crate::{
    SessionEvent,
    modules::{
        executor::{self, BoxFuture, JoinHandle},
        moqt::{
            control_plane::{
                constants::TerminationErrorCode,
                control_messages::key_value_pair::KeyValuePair,
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
                error_codes::{DataStreamResetCode, PublishDoneCode, RequestErrorCode},
                handler::{
                    fetch_cancel_handler::FetchCancelHandler,
                    fetch_handler::FetchHandler,
                    go_away_handler::GoAwayHandler,
                    publish_done_handler::PublishDoneHandler,
                    publish_handler::PublishHandler,
                    publish_namespace_cancel_handler::PublishNamespaceCancelHandler,
                    publish_namespace_done_handler::PublishNamespaceDoneHandler,
                    publish_namespace_handler::PublishNamespaceHandler,
                    subscribe_handler::SubscribeHandler,
                    subscribe_namespace_handler::SubscribeNamespaceHandler,
                    subscribe_update_handler::{SubscribeUpdateHandler, UpdateAnswerReceiver},
                    track_status_handler::TrackStatusHandler,
                    unsubscribe_handler::UnsubscribeHandler,
                    unsubscribe_namespace_handler::UnsubscribeNamespaceHandler,
                },
            },
            data_plane::stream::{
                stream_receiver::StreamReceiveError, stream_sender::StreamSender,
            },
            domains::{connection::Connection, session_context::SessionContext},
            version,
        },
        transport::read_error::ReadError,
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

/// §10.1: every request, REQUEST_UPDATE included, takes a new Request ID of
/// the peer's parity; `false` after closing the session when it does not.
fn accept_request_id(scope: &Scope, request_id: RequestId) -> bool {
    if scope.channel().accept_peer_request_id(request_id) {
        return true;
    }
    tracing::error!(
        request_id,
        "invalid or repeated Request ID; closing session"
    );
    scope.session.close_connection(
        &scope.connection,
        TerminationErrorCode::InvalidRequestId,
        "invalid request id",
    );
    false
}

/// Why the track of a message carrying `properties` is not processed.
fn refused_track(properties: &[KeyValuePair]) -> Option<(RequestErrorCode, &'static str)> {
    if messages::has_malformed_immutable_properties(properties) {
        Some((
            RequestErrorCode::MalformedTrack,
            "malformed Immutable Properties",
        ))
    } else if has_mandatory_track_property(properties) {
        Some((
            RequestErrorCode::UnsupportedExtension,
            "mandatory track property not understood",
        ))
    } else {
        None
    }
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
            Err(StreamReceiveError::Closed(ReadError::ConnectionLost)) => {
                tracing::debug!("control stream ended with the session");
                return;
            }
            Ok(None) | Err(StreamReceiveError::Closed(_)) => {
                close_on_violation(&scope, "the control stream closed");
                return;
            }
            Err(
                StreamReceiveError::Decode(reason) | StreamReceiveError::ProtocolViolation(reason),
            ) => {
                close_on_violation(&scope, &reason);
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
                    Arc::new(StreamSender::new(send_stream)),
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
    fn run(connection: Weak<Connection>, sender: Arc<StreamSender>, receiver: MessageReceiver) {
        executor::spawn(
            "draft-18 incoming request",
            Self::serve(connection, sender, receiver).in_current_span(),
        );
    }

    async fn serve(
        connection: Weak<Connection>,
        sender: Arc<StreamSender>,
        mut receiver: MessageReceiver,
    ) {
        let first = receiver.receive().await;
        let Some(context) = Scope::of(&connection) else {
            return;
        };
        let message = match first {
            Ok(Some(message)) => message,
            Err(
                StreamReceiveError::Decode(reason) | StreamReceiveError::ProtocolViolation(reason),
            ) => {
                close_on_violation(&context, &reason);
                return;
            }
            Ok(None) | Err(StreamReceiveError::Closed(_)) => return,
        };
        let Some((request_id, kind, mut baseline)) =
            Self::accept_request(&context, &sender, message).await
        else {
            return;
        };
        drop(context);
        let mut update_answers = UpdateAnswers::default();
        loop {
            let received = tokio::select! {
                received = receiver.receive() => received,
                answer = update_answers.next() => {
                    send_update_answer(&sender, answer).await;
                    continue;
                }
            };
            let Some(context) = Scope::of(&connection) else {
                return;
            };
            match received {
                Ok(Some(message)) => {
                    match Self::follow_up(
                        &context,
                        &sender,
                        request_id,
                        &kind,
                        &mut baseline,
                        message,
                    )
                    .await
                    {
                        FollowUp::Continue => {}
                        FollowUp::AnswerUpdate(answer_receiver) => {
                            update_answers.push(answer_receiver);
                        }
                        FollowUp::Done => return,
                    }
                }
                Err(StreamReceiveError::Closed(ReadError::ConnectionLost)) => return,
                // §3.3.2: a FIN does not cancel a FETCH; its stream ends once
                // the response is done (§5.2).
                Ok(None) if kind == RequestStreamKind::Fetch => return,
                Ok(None) | Err(StreamReceiveError::Closed(_)) => {
                    Self::peer_ended(&context, request_id, kind);
                    return;
                }
                Err(
                    StreamReceiveError::Decode(reason)
                    | StreamReceiveError::ProtocolViolation(reason),
                ) => {
                    close_on_violation(&context, &reason);
                    return;
                }
            }
        }
    }

    /// Registers the peer's request and hands it to the application as the
    /// event draft-14 would raise. `None` when the stream is done.
    async fn accept_request(
        scope: &Scope,
        sender: &Arc<StreamSender>,
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
        if !accept_request_id(scope, request_id) {
            return None;
        }
        let mut baseline = None;
        let mut default_publisher_priority = messages::DEFAULT_PUBLISHER_PRIORITY;
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
                if let Some((code, _)) = refused_track(&track_properties) {
                    Self::reject(sender, code).await;
                    return None;
                }
                default_publisher_priority =
                    messages::default_publisher_priority(&track_properties);
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
            Message::SubscribeTracks(subscribe_tracks) => (
                RequestStreamKind::SubscribeTracks(subscribe_tracks.track_namespace_prefix.clone()),
                SessionEvent::SubscribeNameSpace(SubscribeNamespaceHandler::new(
                    scope.connection.clone(),
                    SubscribeNamespace {
                        request_id: subscribe_tracks.request_id,
                        track_namespace_prefix: subscribe_tracks.track_namespace_prefix,
                        authorization_token: subscribe_tracks.authorization_tokens,
                    },
                )),
            ),
            _ => unreachable!("non-request messages returned above"),
        };
        // Registered before the event, so a handler can answer at once.
        channel.requests().incoming.insert(
            request_id,
            RequestStream {
                kind: kind.clone(),
                sender: sender.clone(),
                default_publisher_priority,
                fetch_end: Default::default(),
            },
        );
        emit(scope, event);
        Some((request_id, kind, baseline))
    }

    async fn reject(sender: &StreamSender, code: RequestErrorCode) {
        let Ok(message) = messages::encode_request_error(code, "") else {
            return;
        };
        if sender.send(&message).await.is_ok() {
            let _ = sender.close().await;
        }
    }

    async fn follow_up(
        scope: &Scope,
        sender: &StreamSender,
        request_id: RequestId,
        kind: &RequestStreamKind,
        baseline: &mut Option<SubscribeUpdate>,
        message: Message,
    ) -> FollowUp {
        if let Message::RequestUpdate(update) = &message
            && !accept_request_id(scope, update.request_id)
        {
            return FollowUp::Done;
        }
        match (kind, message) {
            (RequestStreamKind::Subscribe, Message::RequestUpdate(update)) => {
                let Some(baseline) = baseline else {
                    unreachable!("a SUBSCRIBE stream keeps its baseline")
                };
                let (update, filter) = apply_update(baseline, update.request_id, update.parameters);
                FollowUp::AnswerUpdate(emit_update(scope, update, filter))
            }
            // §10.14: the subscriber cannot update a TRACK_STATUS.
            (RequestStreamKind::TrackStatus, Message::RequestUpdate(_)) => {
                close_on_violation(scope, "REQUEST_UPDATE on a TRACK_STATUS stream");
                FollowUp::Done
            }
            (_, Message::RequestUpdate(_)) => {
                // Only a subscription has a Largest Object.
                send_update_answer(sender, Some(None)).await;
                FollowUp::Continue
            }
            // Answers to our REQUEST_UPDATE of the subscription the PUBLISH
            // started (§10.9); a failed one is followed by PUBLISH_DONE.
            (RequestStreamKind::Publish, Message::RequestOk(_) | Message::RequestError(_)) => {
                FollowUp::Continue
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
                FollowUp::Done
            }
            (_, Message::GoAway(_)) => {
                tracing::debug!(request_id, "per-request GOAWAY ignored");
                FollowUp::Continue
            }
            _ => {
                close_on_violation(scope, "unexpected message on a request stream");
                FollowUp::Done
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

/// What reading the next message of a peer's request stream leads to.
enum FollowUp {
    Continue,
    /// Answer the REQUEST_UPDATE once its handler is answered or dropped,
    /// while the stream goes on being read.
    AnswerUpdate(UpdateAnswerReceiver),
    Done,
}

fn emit_update(
    scope: &Scope,
    update: SubscribeUpdate,
    filter: Option<FilterType>,
) -> UpdateAnswerReceiver {
    let (handler, answer_receiver) =
        SubscribeUpdateHandler::answered(&scope.connection, update, filter);
    emit(scope, SessionEvent::SubscribeUpdate(handler));
    answer_receiver
}

/// The pending answers to one request stream's REQUEST_UPDATEs, yielded in
/// arrival order (§10.9: exactly one REQUEST_OK or REQUEST_ERROR each). Each
/// waits for its handler at most the control message timeout, counted from
/// its arrival.
#[derive(Default)]
struct UpdateAnswers(FuturesOrdered<BoxFuture<Option<Option<Location>>>>);

impl UpdateAnswers {
    fn push(&mut self, answer_receiver: UpdateAnswerReceiver) {
        self.0.push_back(Box::pin(
            answer_receiver.within(version::CONTROL_MESSAGE_RESPONSE_TIMEOUT),
        ));
    }

    /// The next answer to write; pending while no update awaits one.
    async fn next(&mut self) -> Option<Option<Location>> {
        match self.0.next().await {
            Some(answer) => answer,
            None => std::future::pending().await,
        }
    }
}

/// REQUEST_OK with the LARGEST_OBJECT `answer` carries, or REQUEST_ERROR
/// TIMEOUT for an update the application did not answer in time (§10.6:
/// "could not be completed before an implementation specific timeout").
async fn send_update_answer(sender: &StreamSender, answer: Option<Option<Location>>) {
    let message = match answer {
        Some(largest) => messages::encode_request_update_ok(largest),
        None => {
            tracing::warn!("REQUEST_UPDATE not answered in time; answering REQUEST_ERROR");
            messages::encode_request_error(RequestErrorCode::Timeout, "update not answered in time")
        }
    };
    if let Ok(message) = message
        && let Err(error) = sender.send(&message).await
    {
        tracing::debug!(%error, "could not answer REQUEST_UPDATE");
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
/// applied to `baseline`. Also returns the SUBSCRIPTION_FILTER the update
/// carries: the baseline cannot hold a relative filter's Start Location.
fn apply_update(
    baseline: &mut SubscribeUpdate,
    request_id: RequestId,
    parameters: Vec<Parameter>,
) -> (SubscribeUpdate, Option<FilterType>) {
    baseline.request_id = request_id;
    baseline.authorization_tokens.clear();
    let mut updated_filter = None;
    for parameter in parameters {
        match parameter {
            Parameter::SubscriberPriority(priority) => baseline.subscriber_priority = priority,
            Parameter::Forward(forward) => baseline.forward = forward,
            Parameter::AuthorizationToken(token) => baseline.authorization_tokens.push(token),
            Parameter::ObjectDeliveryTimeout(timeout) => baseline.delivery_timeout = Some(timeout),
            Parameter::SubscriptionFilter(filter) => {
                (baseline.start_location, baseline.end_group) = filter_range(filter);
                updated_filter = Some(filter);
            }
            _ => {}
        }
    }
    (baseline.clone(), updated_filter)
}

pub(crate) struct OutgoingRequestTask {
    pub(super) connection: Weak<Connection>,
    pub(super) request_id: RequestId,
    pub(super) kind: RequestStreamKind,
    pub(super) sender: Arc<StreamSender>,
    pub(super) receiver: MessageReceiver,
    pub(super) response: tokio::sync::oneshot::Sender<ResponseMessage>,
    pub(super) group_order: GroupOrder,
}

impl OutgoingRequestTask {
    /// Drops the objects buffered under the alias of a SUBSCRIBE_OK whose
    /// caller stopped waiting; the stream reset already withdrew the request
    /// (§3.3.2).
    pub(super) async fn abandoned(scope: &Scope, response: ResponseMessage) {
        if let ResponseMessage::SubscribeOk(subscribe_ok) = response {
            scope
                .session
                .cancel_unclaimed_track_alias(&scope.connection, subscribe_ok.track_alias)
                .await;
        }
    }

    pub(crate) fn run(self) {
        executor::spawn("draft-18 outgoing request", self.read().in_current_span());
    }

    async fn read(self) {
        let Self {
            connection,
            request_id,
            kind,
            sender,
            mut receiver,
            response,
            group_order,
        } = self;
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
        if let Message::SubscribeOk(ok) = &message {
            context.channel().set_default_publisher_priority(
                request_id,
                messages::default_publisher_priority(&ok.track_properties),
            );
        }
        let Some(first_response) = Self::first_response(request_id, &kind, message, group_order)
        else {
            close_on_violation(&context, "the first response does not match the request");
            return;
        };
        let succeeded = is_success(&first_response);
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
        if let Err(abandoned) = response.send(first_response) {
            Self::abandoned(&context, abandoned).await;
            return;
        }
        if !succeeded {
            return;
        }
        drop(context);
        let mut publish_done = false;
        // Namespaces a SUBSCRIBE_NAMESPACE stream announced and not withdrawn.
        let mut namespaces: HashSet<Vec<String>> = HashSet::new();
        let mut update_answers = UpdateAnswers::default();
        loop {
            let received = tokio::select! {
                received = receiver.receive() => received,
                answer = update_answers.next() => {
                    send_update_answer(&sender, answer).await;
                    continue;
                }
            };
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
                    if !accept_request_id(&context, update.request_id) {
                        return;
                    }
                    let Some(baseline) = &mut baseline else {
                        unreachable!("an accepted PUBLISH keeps its baseline")
                    };
                    let (update, filter) =
                        apply_update(baseline, update.request_id, update.parameters);
                    update_answers.push(emit_update(&context, update, filter));
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
                    Self::withdraw_namespaces(&context, namespaces);
                    Self::peer_ended(&context, request_id, kind, publish_done, true).await;
                    return;
                }
                Ok(Some(_)) => {
                    close_on_violation(&context, "unexpected message on a request stream");
                    return;
                }
                Err(
                    StreamReceiveError::Decode(reason)
                    | StreamReceiveError::ProtocolViolation(reason),
                ) => {
                    close_on_violation(&context, &reason);
                    return;
                }
                Err(StreamReceiveError::Closed(ReadError::ConnectionLost)) => return,
                Ok(None) | Err(StreamReceiveError::Closed(_)) => {
                    Self::withdraw_namespaces(&context, namespaces);
                    Self::peer_ended(&context, request_id, kind, publish_done, false).await;
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

    /// §10.18: the end of a SUBSCRIBE_NAMESPACE request withdraws every
    /// namespace it announced.
    fn withdraw_namespaces(scope: &Scope, namespaces: HashSet<Vec<String>>) {
        for namespace in namespaces {
            Self::namespace_withdrawn(scope, namespace);
        }
    }

    fn namespace_withdrawn(scope: &Scope, namespace: Vec<String>) {
        emit(
            scope,
            SessionEvent::PublishNamespaceDone(PublishNamespaceDoneHandler::new(
                PublishNamespaceDone::new(namespace),
            )),
        );
    }

    fn first_response(
        request_id: RequestId,
        kind: &RequestStreamKind,
        message: Message,
        group_order: GroupOrder,
    ) -> Option<ResponseMessage> {
        Some(match (kind, message) {
            (RequestStreamKind::Subscribe, Message::SubscribeOk(ok)) => {
                match refused_track(&ok.track_properties) {
                    Some((code, reason)) => {
                        ResponseMessage::SubscribeError(request_id, code, reason.to_string())
                    }
                    None => ResponseMessage::SubscribeOk(ok.into_subscribe_ok(request_id)),
                }
            }
            (RequestStreamKind::Subscribe, Message::RequestError(error)) => {
                ResponseMessage::SubscribeError(request_id, error.code, error.reason)
            }
            (RequestStreamKind::TrackStatus, Message::RequestOk(ok)) => {
                ResponseMessage::TrackStatusOk(ok.into_track_status_ok(request_id))
            }
            (RequestStreamKind::TrackStatus, Message::RequestError(error)) => {
                ResponseMessage::TrackStatusError(request_id, error.code, error.reason)
            }
            (RequestStreamKind::Fetch, Message::FetchOk(ok)) => {
                match refused_track(&ok.track_properties) {
                    Some((code, reason)) => {
                        ResponseMessage::FetchError(request_id, code, reason.to_string())
                    }
                    None => ResponseMessage::FetchOk(ok.into_fetch_ok(request_id, group_order)),
                }
            }
            (RequestStreamKind::Fetch, Message::RequestError(error)) => {
                ResponseMessage::FetchError(request_id, error.code, error.reason)
            }
            (RequestStreamKind::Publish, Message::RequestOk(ok)) => {
                ResponseMessage::PublishOk(ok.into_publish_ok(request_id))
            }
            (RequestStreamKind::Publish, Message::RequestError(error)) => {
                ResponseMessage::PublishError(request_id, error.code, error.reason)
            }
            (RequestStreamKind::PublishNamespace(_), Message::RequestOk(_)) => {
                ResponseMessage::PublishNamespaceOk(request_id)
            }
            (RequestStreamKind::PublishNamespace(_), Message::RequestError(error)) => {
                ResponseMessage::PublishNamespaceError(request_id, error.code, error.reason)
            }
            (RequestStreamKind::SubscribeNamespace(_), Message::RequestOk(_)) => {
                ResponseMessage::SubscribeNameSpaceOk(request_id)
            }
            (RequestStreamKind::SubscribeNamespace(_), Message::RequestError(error)) => {
                ResponseMessage::SubscribeNameSpaceError(request_id, error.code, error.reason)
            }
            _ => return None,
        })
    }

    /// The peer ended a request this endpoint still had open: raises the
    /// withdrawal draft-14 would have sent as a message, then ends this side's
    /// half with FIN. With `going_away` (a GOAWAY on the request stream,
    /// §10.4) the request ends with GOING_AWAY and its stream is reset, as
    /// draft-14 events have no per-request migration.
    async fn peer_ended(
        scope: &Scope,
        request_id: RequestId,
        kind: RequestStreamKind,
        publish_done: bool,
        going_away: bool,
    ) {
        let Some(stream) = scope.channel().take_outgoing(request_id) else {
            return;
        };
        let (publish_done_code, request_error_code, reason) = if going_away {
            (
                PublishDoneCode::GoingAway,
                RequestErrorCode::GoingAway,
                "request moved by GOAWAY",
            )
        } else {
            (
                PublishDoneCode::InternalError,
                RequestErrorCode::InternalError,
                "request stream ended",
            )
        };
        let event = match kind {
            RequestStreamKind::Subscribe if !publish_done => Some(SessionEvent::PublishDone(
                PublishDoneHandler::new(request_id, publish_done_code, 0, reason.to_string()),
            )),
            RequestStreamKind::Publish => Some(SessionEvent::Unsubscribe(UnsubscribeHandler::new(
                scope.session.clone(),
                &scope.connection,
                Unsubscribe { request_id },
            ))),
            RequestStreamKind::PublishNamespace(namespace) => Some(
                SessionEvent::PublishNamespaceCancel(PublishNamespaceCancelHandler::new(
                    namespace.join("/"),
                    request_error_code,
                    reason.to_string(),
                )),
            ),
            _ => None,
        };
        if let Some(event) = event {
            emit(scope, event);
        }
        let ended = if going_away {
            stream
                .sender
                .reset(super::error_codes::stream_reset_code_to_wire(
                    DataStreamResetCode::GoingAway,
                ))
                .await
        } else {
            stream.sender.close().await
        };
        if let Err(error) = ended {
            tracing::debug!(%error, "could not end the stream of a request the peer ended");
        }
    }
}

//! draft-18 (docs/spec/draft-ietf-moq-transport-18.txt). Each endpoint
//! keeps a unidirectional control stream (SETUP, then GOAWAY); every
//! request opens its own bidirectional stream, which carries the response
//! and ends with the request: cancelling a request resets its stream
//! (§3.3, §3.3.2).

pub(crate) mod codec;
pub(crate) mod error_codes;
pub(crate) mod message_stream;
pub(crate) mod messages;
pub(crate) mod objects;
pub(crate) mod parameters;
pub(crate) mod request_streams;
pub(crate) mod setup;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use bytes::BytesMut;

use crate::{
    RequestFailure,
    modules::{
        executor,
        moqt::{
            control_plane::{
                control_messages::messages::parameters::group_order::GroupOrder,
                enums::{RequestId, ResponseMessage},
                error_codes::DataStreamResetCode,
            },
            data_plane::stream::stream_sender::StreamSender,
            domains::{connection::Connection, session_context::SessionContext},
            version::{self, Notification, OutgoingRequest, OutgoingResponse},
        },
        transport::transport_send_stream::TransportSendError,
    },
};

use message_stream::{MessageDecoder, MessageReceiver};

/// What the request tasks need beyond the connection, known once the
/// control receiver starts.
struct Binding {
    session: Weak<SessionContext>,
    publish_done_sender: tokio::sync::mpsc::UnboundedSender<RequestId>,
}

/// A request task's connection and the session it belongs to.
pub(crate) struct Scope {
    pub(crate) session: Arc<SessionContext>,
    pub(crate) connection: Arc<Connection>,
}

impl Scope {
    /// `None` once the connection or its session is gone.
    pub(crate) fn of(connection: &Weak<Connection>) -> Option<Self> {
        let connection = connection.upgrade()?;
        let session = ControlChannel::of(&connection)
            .binding
            .get()?
            .session
            .upgrade()?;
        Some(Self {
            session,
            connection,
        })
    }

    fn channel(&self) -> &ControlChannel {
        ControlChannel::of(&self.connection)
    }
}

/// What a request stream was opened for; decides which message a
/// notification becomes and which event the end of the stream raises.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RequestStreamKind {
    Subscribe,
    TrackStatus,
    Fetch,
    Publish,
    PublishNamespace(Vec<String>),
    SubscribeNamespace(Vec<String>),
    /// Only incoming: the draft-14 model has no SUBSCRIBE_TRACKS of its own,
    /// so it reaches the application as SUBSCRIBE_NAMESPACE, whose draft-14
    /// meaning includes the PUBLISH fan-out (draft-14 §9.28).
    SubscribeTracks(Vec<String>),
}

impl RequestStreamKind {
    fn namespace_prefix_of(&self, namespace: &[String]) -> Option<&[String]> {
        match self {
            Self::SubscribeNamespace(prefix) | Self::SubscribeTracks(prefix)
                if namespace.starts_with(prefix) =>
            {
                Some(prefix)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct RequestStream {
    kind: RequestStreamKind,
    sender: Arc<StreamSender>,
}

#[derive(Default)]
struct Requests {
    /// Streams of requests this endpoint sent.
    outgoing: HashMap<RequestId, RequestStream>,
    /// Streams of requests the peer sent.
    incoming: HashMap<RequestId, RequestStream>,
    peer_request_ids: HashSet<RequestId>,
    /// Namespaces sent as NAMESPACE, with the SUBSCRIBE_NAMESPACE stream
    /// that carried them.
    advertised: HashMap<Vec<String>, RequestId>,
}

/// Request IDs this side makes up for events that need no response (a
/// NAMESPACE raised as PUBLISH_NAMESPACE) count down from here, far above
/// any the peer would use, so they cannot collide with real ones.
const FIRST_SYNTHETIC_REQUEST_ID: RequestId = u64::MAX;

pub(crate) struct ControlChannel {
    control: StreamSender,
    request_id: AtomicU64,
    synthetic_request_id: AtomicU64,
    /// The parity of the peer's Request IDs (§10.1).
    peer_parity: u64,
    requests: std::sync::Mutex<Requests>,
    binding: std::sync::OnceLock<Binding>,
}

fn cancelled() -> u64 {
    error_codes::stream_reset_code_to_wire(DataStreamResetCode::Cancelled)
}

/// Withdraws a request by resetting its stream (§3.3.2).
async fn cancel(stream: Option<RequestStream>) -> Result<(), TransportSendError> {
    match stream {
        Some(stream) => stream.sender.reset(cancelled()).await,
        None => Ok(()),
    }
}

fn encode_error(error: anyhow::Error) -> TransportSendError {
    TransportSendError::SessionError {
        reason: error.to_string(),
    }
}

impl ControlChannel {
    fn new(control: StreamSender, first_request_id: RequestId) -> Self {
        Self {
            control,
            request_id: AtomicU64::new(first_request_id),
            synthetic_request_id: AtomicU64::new(FIRST_SYNTHETIC_REQUEST_ID),
            peer_parity: 1 - first_request_id,
            requests: std::sync::Mutex::new(Requests::default()),
            binding: std::sync::OnceLock::new(),
        }
    }

    fn of(connection: &Connection) -> &Self {
        let version::ControlChannel::Draft18(channel) = &connection.control else {
            unreachable!("draft-18 tasks run only on draft-18 connections")
        };
        channel
    }

    fn bind(
        &self,
        session: Weak<SessionContext>,
        publish_done_sender: tokio::sync::mpsc::UnboundedSender<RequestId>,
    ) {
        let _ = self.binding.set(Binding {
            session,
            publish_done_sender,
        });
    }

    fn publish_done_sender(&self) -> Option<&tokio::sync::mpsc::UnboundedSender<RequestId>> {
        self.binding
            .get()
            .map(|binding| &binding.publish_done_sender)
    }

    fn requests(&self) -> std::sync::MutexGuard<'_, Requests> {
        self.requests
            .lock()
            .expect("draft-18 request registry poisoned")
    }

    pub(crate) fn next_request_id(&self) -> RequestId {
        self.request_id.fetch_add(2, Ordering::SeqCst)
    }

    /// The Request ID after the largest one the peer has used, for GOAWAY.
    fn next_peer_request_id(&self) -> RequestId {
        self.requests()
            .peer_request_ids
            .iter()
            .max()
            .map_or(self.peer_parity, |largest| largest + 2)
    }

    /// `false` for a Request ID of the wrong parity or one already used
    /// (§10.1).
    fn accept_peer_request_id(&self, request_id: RequestId) -> bool {
        request_id % 2 == self.peer_parity && self.requests().peer_request_ids.insert(request_id)
    }

    fn synthetic_request_id(&self) -> RequestId {
        self.synthetic_request_id.fetch_sub(1, Ordering::SeqCst)
    }

    fn is_synthetic(&self, request_id: RequestId) -> bool {
        request_id > self.synthetic_request_id.load(Ordering::SeqCst)
    }

    fn forget_advertised(&self, subscription_request_id: RequestId) {
        self.requests()
            .advertised
            .retain(|_, request_id| *request_id != subscription_request_id);
    }

    /// The peer's namespace subscription `namespace` falls under. A peer can
    /// hold SUBSCRIBE_NAMESPACE and SUBSCRIBE_TRACKS for the same prefix, and
    /// only the former takes NAMESPACE, so it comes first.
    fn namespace_subscription(&self, namespace: &[String]) -> Option<(RequestId, RequestStream)> {
        self.requests()
            .incoming
            .iter()
            .filter(|(_, stream)| stream.kind.namespace_prefix_of(namespace).is_some())
            .max_by_key(|(_, stream)| {
                matches!(stream.kind, RequestStreamKind::SubscribeNamespace(_))
            })
            .map(|(request_id, stream)| (*request_id, stream.clone()))
    }

    /// draft-14 announces a namespace to a namespace subscriber with
    /// PUBLISH_NAMESPACE; draft-18 sends NAMESPACE on the subscription's
    /// stream instead (§6.1), and nothing on a SUBSCRIBE_TRACKS stream. The
    /// announcement is accepted at once, as no response follows.
    async fn advertise_namespace(
        &self,
        request_id: RequestId,
        namespace: &[String],
    ) -> Result<Option<PendingResponse>, RequestFailure> {
        let Some((subscription_request_id, stream)) = self.namespace_subscription(namespace) else {
            return Ok(None);
        };
        if let RequestStreamKind::SubscribeNamespace(prefix) = &stream.kind {
            let message =
                messages::encode_namespace(messages::NAMESPACE, &namespace[prefix.len()..])
                    .map_err(|_| {
                        RequestFailure::ProtocolViolation("namespace could not be encoded")
                    })?;
            stream.sender.send(&message).await?;
            self.requests()
                .advertised
                .insert(namespace.to_vec(), subscription_request_id);
        }
        Ok(Some(PendingResponse::resolved(
            ResponseMessage::PublishNamespaceOk(request_id),
        )))
    }

    fn take_incoming(&self, request_id: RequestId) -> Option<RequestStream> {
        self.requests().incoming.remove(&request_id)
    }

    fn take_outgoing(&self, request_id: RequestId) -> Option<RequestStream> {
        self.requests().outgoing.remove(&request_id)
    }

    fn take_by_kind(
        map: &mut HashMap<RequestId, RequestStream>,
        kind: &RequestStreamKind,
    ) -> Option<RequestStream> {
        let request_id = *map.iter().find(|(_, stream)| &stream.kind == kind)?.0;
        map.remove(&request_id)
    }

    // On wasm32 the browser transport is !Send, but the context is shared with
    // the native build, where the tasks run on a multi-threaded executor.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub(super) async fn send_request(
        &self,
        connection: &Arc<Connection>,
        request: OutgoingRequest,
    ) -> Result<PendingResponse, RequestFailure> {
        let encoding_failed = |error: anyhow::Error| {
            tracing::error!(%error, "a request could not be encoded for draft-18");
            RequestFailure::ProtocolViolation("request could not be encoded")
        };
        if let OutgoingRequest::PublishNamespace(publish_namespace) = &request
            && let Some(accepted) = self
                .advertise_namespace(
                    publish_namespace.request_id,
                    &publish_namespace.track_namespace,
                )
                .await?
        {
            return Ok(accepted);
        }
        let (request_id, kind, message, group_order) = match request {
            OutgoingRequest::Subscribe(subscribe) => (
                subscribe.request_id,
                RequestStreamKind::Subscribe,
                messages::encode_subscribe(&subscribe).map_err(encoding_failed)?,
                None,
            ),
            OutgoingRequest::TrackStatus(track_status) => (
                track_status.request_id,
                RequestStreamKind::TrackStatus,
                messages::encode_track_status(&track_status).map_err(encoding_failed)?,
                None,
            ),
            OutgoingRequest::Fetch(fetch) => (
                fetch.request_id,
                RequestStreamKind::Fetch,
                messages::encode_fetch(&fetch).map_err(encoding_failed)?,
                Some(fetch.group_order),
            ),
            OutgoingRequest::Publish(publish) => (
                publish.request_id,
                RequestStreamKind::Publish,
                messages::encode_publish(&publish).map_err(encoding_failed)?,
                None,
            ),
            OutgoingRequest::PublishNamespace(publish_namespace) => (
                publish_namespace.request_id,
                RequestStreamKind::PublishNamespace(publish_namespace.track_namespace.clone()),
                messages::encode_publish_namespace(&publish_namespace).map_err(encoding_failed)?,
                None,
            ),
            OutgoingRequest::SubscribeNamespace(subscribe_namespace) => (
                subscribe_namespace.request_id,
                RequestStreamKind::SubscribeNamespace(
                    subscribe_namespace.track_namespace_prefix.clone(),
                ),
                messages::encode_subscribe_namespace(&subscribe_namespace)
                    .map_err(encoding_failed)?,
                None,
            ),
        };
        let (send_stream, receive_stream) = connection
            .transport
            .open_bi()
            .await
            .map_err(|_| RequestFailure::SessionClosed)?;
        let sender = Arc::new(StreamSender::new(send_stream));
        sender.send(&message).await?;
        self.requests().outgoing.insert(
            request_id,
            RequestStream {
                kind: kind.clone(),
                sender: sender.clone(),
            },
        );
        let (response_sender, receiver) = tokio::sync::oneshot::channel();
        request_streams::OutgoingRequestTask {
            connection: Arc::downgrade(connection),
            request_id,
            kind,
            sender,
            receiver: MessageReceiver::new(receive_stream, MessageDecoder),
            response: response_sender,
            group_order: group_order.unwrap_or(GroupOrder::Ascending),
        }
        .run();
        Ok(PendingResponse {
            receiver,
            cancel: Some((Arc::downgrade(connection), request_id)),
        })
    }

    /// A response to a synthetic Request ID goes nowhere.
    async fn send_on_incoming(
        &self,
        request_id: RequestId,
        message: anyhow::Result<BytesMut>,
        ends_request: bool,
    ) -> Result<(), TransportSendError> {
        if self.is_synthetic(request_id) {
            return Ok(());
        }
        let message = message.map_err(encode_error)?;
        let stream = if ends_request {
            self.take_incoming(request_id)
        } else {
            self.requests().incoming.get(&request_id).cloned()
        };
        let Some(stream) = stream else {
            tracing::debug!(request_id, "no open request stream for the response");
            return Err(TransportSendError::ClosedStream);
        };
        stream.sender.send(&message).await?;
        if ends_request {
            stream.sender.close().await?;
        }
        Ok(())
    }

    pub(super) async fn respond(
        &self,
        response: OutgoingResponse,
    ) -> Result<(), TransportSendError> {
        match response {
            OutgoingResponse::SubscribeOk(ok) => {
                let message = messages::encode_subscribe_ok(&ok);
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::TrackStatusOk(ok) => {
                let message = messages::encode_track_status_ok(&ok);
                // §10.14: the stream ends with the response.
                self.send_on_incoming(ok.request_id, message, true).await
            }
            OutgoingResponse::PublishOk(ok) => {
                let message = messages::encode_publish_ok(&ok);
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::FetchOk(ok) => {
                let message = messages::encode_fetch_ok(&ok);
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::PublishNamespaceOk(ok)
            | OutgoingResponse::SubscribeNamespaceOk(ok) => {
                let message = messages::encode_empty_request_ok();
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::Error {
                request_id,
                code,
                reason_phrase,
                ..
            } => {
                let message = messages::encode_request_error(code, &reason_phrase);
                // §3.3.2: a rejection ends with FIN.
                self.send_on_incoming(request_id, message, true).await
            }
        }
    }

    pub(super) async fn notify(
        &self,
        notification: Notification,
    ) -> Result<(), TransportSendError> {
        match notification {
            Notification::Unsubscribe(unsubscribe) => {
                // Either our SUBSCRIBE or a subscription the peer's PUBLISH
                // started.
                cancel(
                    self.take_outgoing(unsubscribe.request_id)
                        .or_else(|| self.take_incoming(unsubscribe.request_id)),
                )
                .await
            }
            Notification::SubscribeUpdate(update) => {
                let stream = {
                    let requests = self.requests();
                    requests
                        .outgoing
                        .get(&update.subscription_request_id)
                        .or_else(|| requests.incoming.get(&update.subscription_request_id))
                        .cloned()
                };
                let Some(stream) = stream else {
                    return Err(TransportSendError::ClosedStream);
                };
                let message = messages::encode_request_update(update.request_id, &update)
                    .map_err(encode_error)?;
                stream.sender.send(&message).await
            }
            Notification::PublishDone {
                request_id,
                code,
                stream_count,
                reason,
            } => {
                // Either a SUBSCRIBE the peer sent or our PUBLISH.
                let stream = self
                    .take_incoming(request_id)
                    .or_else(|| self.take_outgoing(request_id));
                let Some(stream) = stream else {
                    return Err(TransportSendError::ClosedStream);
                };
                let message = messages::encode_publish_done(code, stream_count, &reason)
                    .map_err(encode_error)?;
                stream.sender.send(&message).await?;
                stream.sender.close().await
            }
            Notification::FetchCancel(fetch_cancel) => {
                cancel(self.take_outgoing(fetch_cancel.request_id)).await
            }
            Notification::PublishNamespaceDone(done) => {
                let advertised = self.requests().advertised.remove(&done.track_namespace);
                if let Some(subscription_request_id) = advertised {
                    let stream = self
                        .requests()
                        .incoming
                        .get(&subscription_request_id)
                        .cloned();
                    if let Some(stream) = stream
                        && let Some(prefix) = stream.kind.namespace_prefix_of(&done.track_namespace)
                    {
                        let message = messages::encode_namespace(
                            messages::NAMESPACE_DONE,
                            &done.track_namespace[prefix.len()..],
                        )
                        .map_err(encode_error)?;
                        stream.sender.send(&message).await?;
                    }
                    return Ok(());
                }
                let kind = RequestStreamKind::PublishNamespace(done.track_namespace);
                let stream = Self::take_by_kind(&mut self.requests().outgoing, &kind);
                cancel(stream).await
            }
            Notification::PublishNamespaceCancel { namespace, .. } => {
                let kind = RequestStreamKind::PublishNamespace(namespace);
                let stream = Self::take_by_kind(&mut self.requests().incoming, &kind);
                cancel(stream).await
            }
            Notification::UnsubscribeNamespace(unsubscribe) => {
                let kind =
                    RequestStreamKind::SubscribeNamespace(unsubscribe.track_namespace_prefix);
                let stream = Self::take_by_kind(&mut self.requests().outgoing, &kind);
                cancel(stream).await
            }
            Notification::GoAway(go_away) => {
                let message = messages::encode_go_away(&messages::GoAwayMessage {
                    new_session_uri: go_away.new_session_uri,
                    request_id: Some(self.next_peer_request_id()),
                })
                .map_err(encode_error)?;
                self.control.send(&message).await
            }
            // draft-18 bounds requests with the QUIC stream limit instead.
            Notification::MaxRequestId(_) | Notification::RequestsBlocked(_) => Ok(()),
        }
    }
}

pub(crate) struct PendingResponse {
    receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    /// The request whose stream is reset when the caller stops waiting
    /// before a success response; that withdraws the request (§3.3.2).
    cancel: Option<(Weak<Connection>, RequestId)>,
}

impl Drop for PendingResponse {
    fn drop(&mut self) {
        let Some((connection, request_id)) = self.cancel.take() else {
            return;
        };
        let Some(connection) = connection.upgrade() else {
            return;
        };
        let Some(stream) = ControlChannel::of(&connection).take_outgoing(request_id) else {
            return;
        };
        executor::spawn("cancel draft-18 request", async move {
            if let Err(error) = stream.sender.reset(cancelled()).await {
                tracing::debug!(%error, "could not cancel an abandoned request");
            }
        });
    }
}

impl PendingResponse {
    /// A response known without asking the peer.
    fn resolved(response: ResponseMessage) -> Self {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let _ = sender.send(response);
        Self {
            receiver,
            cancel: None,
        }
    }

    /// Waits at most `CONTROL_MESSAGE_RESPONSE_TIMEOUT`; a timeout or an
    /// error response resets the request stream.
    pub(super) async fn response(mut self) -> Result<ResponseMessage, RequestFailure> {
        match executor::timeout(
            version::CONTROL_MESSAGE_RESPONSE_TIMEOUT,
            &mut self.receiver,
        )
        .await
        {
            Ok(Ok(response)) => {
                if is_success(&response) {
                    self.cancel = None;
                }
                Ok(response)
            }
            Ok(Err(_)) => Err(RequestFailure::SessionClosed),
            Err(_) => Err(RequestFailure::Timeout),
        }
    }
}

fn is_success(response: &ResponseMessage) -> bool {
    !matches!(
        response,
        ResponseMessage::SubscribeError(..)
            | ResponseMessage::PublishError(..)
            | ResponseMessage::FetchError(..)
            | ResponseMessage::TrackStatusError(..)
            | ResponseMessage::PublishNamespaceError(..)
            | ResponseMessage::SubscribeNameSpaceError(..)
    )
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use super::{
        ControlChannel, Scope, codec,
        message_stream::{MessageDecoder, MessageReceiver},
        messages, parameters,
        request_streams::OutgoingRequestTask,
    };

    use crate::{
        RequestErrorCode, RequestFailure, SessionEvent, TrackWriter,
        fetch::{Fetch, FetchOption},
        modules::{
            moqt::{
                control_plane::{
                    control_messages::messages::{
                        parameters::group_order::GroupOrder, subscribe::Subscribe,
                        subscribe_ok::SubscribeOk,
                    },
                    enums::ResponseMessage,
                },
                domains::connection::IncomingObjectNotification,
            },
            moqt::{data_plane::stream::stream_sender::StreamSender, version::Version},
            test_support::{
                DISCONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, accept_publish, datagram_object,
                notify_datagram, published_track_between, read_fetch_objects,
                spawn_connected_draft18_sessions, subscribed_track_reader,
            },
        },
        namespace::SubscribeNamespaceHandler,
        object::{
            ContentExists, DataReceiver, ForwardingPreference, Location, Object, ObjectStatus,
        },
        publish::{PublishDoneCode, PublishOption},
        subscribe::{FilterType, SubscribeHandler, SubscribeOption, SubscribeUpdateHandler},
    };

    const SCHEMES: [&str; 2] = ["moqt", "https"];

    /// More requests than quinn's default limit of 100 concurrent
    /// bidirectional streams, each holding its request stream open.
    const OPEN_REQUESTS: usize = 150;

    #[tokio::test]
    async fn more_than_a_hundred_requests_stay_open_at_once() {
        for scheme in SCHEMES {
            // Arrange
            let (client, server) = spawn_connected_draft18_sessions(
                &format!("draft18-open-requests-{scheme}"),
                scheme,
            )
            .await;
            let answering = tokio::spawn(async move {
                for _ in 0..OPEN_REQUESTS {
                    let SessionEvent::Subscribe(handler) = next_event(&server).await else {
                        panic!("expected SUBSCRIBE");
                    };
                    handler.ok(0, ContentExists::False).await.unwrap();
                }
                server
            });

            // Act
            let subscriptions = tokio::time::timeout(
                HANDSHAKE_TIMEOUT,
                futures::future::join_all((0..OPEN_REQUESTS).map(|index| {
                    let mut subscriber = client.subscriber();
                    async move {
                        subscriber
                            .subscribe(
                                "ns".into(),
                                format!("track-{index}"),
                                SubscribeOption::default(),
                            )
                            .await
                    }
                })),
            )
            .await;

            // Assert
            let subscriptions = subscriptions.expect("every SUBSCRIBE is answered in time");
            assert!(subscriptions.iter().all(Result::is_ok), "{scheme}");
            drop(answering.await.unwrap());
        }
    }

    #[tokio::test]
    async fn more_than_a_hundred_requests_from_the_server_stay_open_at_once() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-open-server-requests", "moqt").await;
        let accepting = tokio::spawn(async move {
            for _ in 0..OPEN_REQUESTS {
                accept_publish(&client).await;
            }
            client
        });

        // Act
        let publications = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            futures::future::join_all((0..OPEN_REQUESTS).map(|index| {
                let publisher = server.publisher();
                async move {
                    publisher
                        .publish(
                            "ns".into(),
                            format!("track-{index}"),
                            PublishOption::default(),
                        )
                        .await
                }
            })),
        )
        .await;

        // Assert
        let publications = publications.expect("every PUBLISH is answered in time");
        assert!(publications.iter().all(Result::is_ok));
        drop(accepting.await.unwrap());
    }

    #[tokio::test]
    async fn a_subscribe_ok_the_caller_gave_up_on_drops_the_objects_of_its_track_alias() {
        // Arrange
        let (_client, server) =
            spawn_connected_draft18_sessions("draft18-abandoned-subscribe-ok", "moqt").await;
        let session = server.subscriber().session;
        let track_alias = 5;
        notify_datagram(&session, track_alias).await;
        let scope = Scope {
            connection: session.connection(),
            session: session.clone(),
        };

        // Act
        OutgoingRequestTask::abandoned(
            &scope,
            ResponseMessage::SubscribeOk(SubscribeOk {
                request_id: 0,
                track_alias,
                expires: 0,
                group_order: crate::object::GroupOrder::Ascending,
                content_exists: ContentExists::False,
                delivery_timeout: None,
                max_duration: None,
            }),
        )
        .await;
        let notification = notify_datagram(&session, track_alias).await;

        // Assert
        assert!(
            matches!(notification, IncomingObjectNotification::Discarded),
            "{notification:?}"
        );
    }

    async fn next_event(session: &crate::Session) -> SessionEvent {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, session.receive_event())
            .await
            .expect("no session event")
            .unwrap()
    }

    /// Spawns a SUBSCRIBE from `client` and returns it with the handler
    /// `server` receives.
    async fn subscribe_requested(
        client: &crate::Session,
        server: &crate::Session,
        option: SubscribeOption,
    ) -> (
        tokio::task::JoinHandle<Result<crate::Subscription, RequestFailure>>,
        SubscribeHandler,
    ) {
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".into(), "track".into(), option)
                .await
        });
        let SessionEvent::Subscribe(handler) = next_event(server).await else {
            panic!("expected SUBSCRIBE");
        };
        (request, handler)
    }

    async fn subscribed(
        client: &crate::Session,
        server: &crate::Session,
        option: SubscribeOption,
    ) -> crate::Subscription {
        let (request, handler) = subscribe_requested(client, server, option).await;
        handler.ok(0, ContentExists::False).await.unwrap();
        request.await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn sessions_offering_only_draft18_speak_draft18_over_quic_and_web_transport() {
        for scheme in SCHEMES {
            // Act
            let (client, server) =
                spawn_connected_draft18_sessions("draft18-negotiate", scheme).await;

            // Assert
            assert_eq!(
                client.publisher().session.connection().control.version(),
                Version::Draft18,
                "{scheme}"
            );
            assert_eq!(
                server.publisher().session.connection().control.version(),
                Version::Draft18,
                "{scheme}"
            );
        }
    }

    #[tokio::test]
    async fn a_published_track_delivers_its_subgroup_objects() {
        for scheme in SCHEMES {
            // Arrange
            let (client, server) =
                spawn_connected_draft18_sessions("draft18-publish", scheme).await;
            let track = published_track_between(client, server).await;
            let mut writer =
                TrackWriter::new(track.client.publisher().create_stream(&track.published), 3);

            // Act
            writer.start_group().await.unwrap();
            writer
                .write(Bytes::from_static(b"a"), vec![])
                .await
                .unwrap();
            writer
                .write(Bytes::from_static(b"b"), vec![])
                .await
                .unwrap();
            writer.finish().await.unwrap();
            let mut reader = subscribed_track_reader(&track.server, &track.accepted).await;
            let first = reader.next_object().await.unwrap().unwrap();
            let second = reader.next_object().await.unwrap().unwrap();

            // Assert
            assert_eq!((first.group_id, first.object_id), (3, 0), "{scheme}");
            assert_eq!(first.payload, Bytes::from_static(b"a"));
            assert_eq!((second.group_id, second.object_id), (3, 1), "{scheme}");
        }
    }

    #[tokio::test]
    async fn a_datagram_reaches_the_subscriber() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-datagram", "moqt").await;
        let track = published_track_between(client, server).await;

        // Act
        track
            .client
            .publisher()
            .create_datagram(&track.published)
            .send_object(&datagram_object())
            .await
            .unwrap();
        let DataReceiver::Datagram(mut receiver) = track
            .server
            .subscriber()
            .accept_data_receiver(&track.accepted)
            .await
            .unwrap()
        else {
            panic!("expected a datagram receiver");
        };
        let object = receiver.receive().await.unwrap();

        // Assert
        assert_eq!(object, datagram_object());
    }

    #[tokio::test]
    async fn a_subscription_is_answered_ended_and_unsubscribed_on_its_stream() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-subscribe", "moqt").await;
        let (request, handler) =
            subscribe_requested(&client, &server, SubscribeOption::default()).await;

        // Act
        let track_alias = handler.ok(0, ContentExists::False).await.unwrap();
        let subscription = request.await.unwrap().unwrap();
        server
            .publisher()
            .publish_done(
                &handler.into_subscription(track_alias),
                PublishDoneCode::TrackEnded,
                0,
                "done".into(),
            )
            .await
            .unwrap();
        let done = next_event(&client).await;

        // Assert
        assert_eq!(subscription.track_alias(), track_alias);
        let SessionEvent::PublishDone(done) = done else {
            panic!("expected PUBLISH_DONE, got {done:?}");
        };
        assert_eq!(done.request_id(), subscription.request_id());
        assert_eq!(done.status_code(), PublishDoneCode::TrackEnded);
    }

    #[tokio::test]
    async fn unsubscribing_resets_the_stream_and_reaches_the_publisher_as_unsubscribe() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-unsubscribe", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;

        // Act
        client
            .subscriber()
            .unsubscribe(&subscription)
            .await
            .unwrap();
        let event = next_event(&server).await;

        // Assert
        let SessionEvent::Unsubscribe(unsubscribe) = event else {
            panic!("expected UNSUBSCRIBE, got {event:?}");
        };
        assert_eq!(unsubscribe.subscribe_id(), subscription.request_id());
    }

    #[tokio::test]
    async fn a_rejected_subscribe_fails_with_its_code() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-rejected", "moqt").await;
        let (request, handler) =
            subscribe_requested(&client, &server, SubscribeOption::default()).await;

        // Act
        handler
            .error(RequestErrorCode::TrackDoesNotExist, "no such track".into())
            .await
            .unwrap();
        let result = request.await.unwrap();

        // Assert
        assert!(
            matches!(
                &result,
                Err(RequestFailure::Rejected {
                    code: RequestErrorCode::TrackDoesNotExist,
                    ..
                })
            ),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_fetch_returns_its_objects() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-fetch", "moqt").await;
        let end = Location {
            group_id: 2,
            object_id: 0,
        };
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .fetch(
                    "ns".into(),
                    "track".into(),
                    Location {
                        group_id: 0,
                        object_id: 0,
                    },
                    end,
                    FetchOption::default(),
                )
                .await
        });
        let SessionEvent::Fetch(handler) = next_event(&server).await else {
            panic!("expected FETCH");
        };
        let objects: Vec<Object> = (0..3)
            .map(|index| Object {
                location: Location {
                    group_id: index / 2,
                    object_id: index % 2,
                },
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
                publisher_priority: 128,
                status: ObjectStatus::Normal,
                extension_headers: Default::default(),
                payload: Bytes::from(vec![index as u8 + 1]),
                ends_group: false,
            })
            .collect();

        // Act
        handler.ok(false, end).await.unwrap();
        let sender = server
            .publisher()
            .create_fetch_stream(handler.request_id)
            .await
            .unwrap();
        for object in &objects {
            sender.send_object(object).await.unwrap();
        }
        sender.close().await.unwrap();
        let fetch_handle = request.await.unwrap().unwrap();
        let mut receiver = client
            .subscriber()
            .accept_fetch_receiver(&fetch_handle)
            .await
            .unwrap();
        let received = read_fetch_objects(&mut receiver).await;

        // Assert
        assert_eq!(received, objects);
        assert!(matches!(receiver.receive().await.unwrap(), Fetch::End));
    }

    #[tokio::test]
    async fn a_namespace_subscriber_hears_namespaces_as_publish_namespace() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-namespaces", "moqt").await;
        let subscriber = client.subscriber();
        let request =
            tokio::spawn(async move { subscriber.subscribe_namespace("live".into()).await });
        let SessionEvent::SubscribeNameSpace(handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE_NAMESPACE");
        };
        handler.ok().await.unwrap();
        request.await.unwrap().unwrap();

        // Act
        server
            .publisher()
            .publish_namespace("live/room".into())
            .await
            .unwrap();
        let announced = next_event(&client).await;
        server
            .publisher()
            .publish_namespace_done("live/room".into())
            .await
            .unwrap();
        let withdrawn = next_event(&client).await;

        // Assert: NAMESPACE / NAMESPACE_DONE arrive as draft-14's events
        let SessionEvent::PublishNamespace(announced) = announced else {
            panic!("expected PUBLISH_NAMESPACE, got {announced:?}");
        };
        assert_eq!(announced.track_namespace, "live/room");
        announced.ok().await.unwrap();
        let SessionEvent::PublishNamespaceDone(withdrawn) = withdrawn else {
            panic!("expected PUBLISH_NAMESPACE_DONE, got {withdrawn:?}");
        };
        assert_eq!(withdrawn.track_namespace(), "live/room");
    }

    /// Sends SUBSCRIBE_TRACKS for `prefix` on a request stream `client`
    /// opens by hand, and returns the stream with the handler `server`
    /// receives.
    async fn subscribe_tracks_requested(
        client: &crate::Session,
        server: &crate::Session,
        prefix: &str,
    ) -> (StreamSender, MessageReceiver, SubscribeNamespaceHandler) {
        let connection = client.publisher().session.connection();
        let (send_stream, receive_stream) = connection.transport.open_bi().await.unwrap();
        let sender = StreamSender::new(send_stream);
        let mut payload = BytesMut::new();
        codec::put_vi64(&mut payload, connection.control.next_request_id());
        codec::put_track_namespace(&mut payload, &[prefix.to_string()]);
        codec::put_vi64(&mut payload, 0);
        sender
            .send(&messages::frame(messages::SUBSCRIBE_TRACKS, &payload).unwrap())
            .await
            .unwrap();
        let event = next_event(server).await;
        let SessionEvent::SubscribeNameSpace(handler) = event else {
            panic!("expected SUBSCRIBE_NAMESPACE, got {event:?}");
        };
        (
            sender,
            MessageReceiver::new(receive_stream, MessageDecoder),
            handler,
        )
    }

    #[tokio::test]
    async fn subscribe_tracks_reaches_the_publisher_as_subscribe_namespace() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-tracks", "moqt").await;
        let (_sender, mut receiver, handler) =
            subscribe_tracks_requested(&client, &server, "live").await;

        // Act
        handler.ok().await.unwrap();
        let response = receiver.receive().await.unwrap();
        let announced = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            server.publisher().publish_namespace("live/room".into()),
        )
        .await;

        // Assert: no NAMESPACE goes to a SUBSCRIBE_TRACKS stream
        assert_eq!(handler.track_namespace_prefix, "live");
        assert!(
            matches!(response, Some(messages::Message::RequestOk(_))),
            "{response:?}"
        );
        assert!(matches!(announced, Ok(Ok(()))), "{announced:?}");
    }

    #[tokio::test]
    async fn a_namespace_subscriber_beside_track_subscribers_hears_the_namespace() {
        // Arrange: 15 SUBSCRIBE_TRACKS on the prefix, so a lookup that could
        // pick one would almost surely do so
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespace-and-tracks", "moqt").await;
        let mut tracks_streams = Vec::new();
        for _ in 0..15 {
            let (sender, receiver, handler) =
                subscribe_tracks_requested(&client, &server, "live").await;
            handler.ok().await.unwrap();
            tracks_streams.push((sender, receiver));
        }
        let subscriber = client.subscriber();
        let request =
            tokio::spawn(async move { subscriber.subscribe_namespace("live".into()).await });
        let SessionEvent::SubscribeNameSpace(handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE_NAMESPACE");
        };
        handler.ok().await.unwrap();
        request.await.unwrap().unwrap();

        // Act
        server
            .publisher()
            .publish_namespace("live/room".into())
            .await
            .unwrap();
        let announced = next_event(&client).await;

        // Assert
        let SessionEvent::PublishNamespace(announced) = announced else {
            panic!("expected PUBLISH_NAMESPACE, got {announced:?}");
        };
        assert_eq!(announced.track_namespace, "live/room");
    }

    #[tokio::test]
    async fn a_goaway_on_a_subscription_ends_it_with_going_away() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-request-goaway", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;
        let server_connection = server.publisher().session.connection();
        let stream = ControlChannel::of(&server_connection)
            .requests()
            .incoming
            .get(&subscription.request_id())
            .cloned()
            .unwrap();
        let go_away = messages::encode_go_away(&messages::GoAwayMessage {
            new_session_uri: String::new(),
            request_id: None,
        })
        .unwrap();

        // Act
        stream.sender.send(&go_away).await.unwrap();
        let event = next_event(&client).await;

        // Assert
        let SessionEvent::PublishDone(done) = event else {
            panic!("expected PUBLISH_DONE, got {event:?}");
        };
        assert_eq!(done.request_id(), subscription.request_id());
        assert_eq!(done.status_code(), PublishDoneCode::GoingAway);
    }

    #[tokio::test]
    async fn a_goaway_on_a_namespace_subscription_withdraws_its_namespaces_once() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespace-goaway", "moqt").await;
        let subscriber = client.subscriber();
        let request =
            tokio::spawn(async move { subscriber.subscribe_namespace("live".into()).await });
        let SessionEvent::SubscribeNameSpace(handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE_NAMESPACE");
        };
        handler.ok().await.unwrap();
        request.await.unwrap().unwrap();
        for namespace in ["live/room", "live/hall"] {
            server
                .publisher()
                .publish_namespace(namespace.into())
                .await
                .unwrap();
            let SessionEvent::PublishNamespace(_) = next_event(&client).await else {
                panic!("expected PUBLISH_NAMESPACE");
            };
        }
        let stream = ControlChannel::of(&server.publisher().session.connection())
            .requests()
            .incoming
            .values()
            .find(|stream| {
                stream.kind == super::RequestStreamKind::SubscribeNamespace(vec!["live".into()])
            })
            .cloned()
            .unwrap();
        let go_away = messages::encode_go_away(&messages::GoAwayMessage {
            new_session_uri: String::new(),
            request_id: None,
        })
        .unwrap();

        // Act
        stream.sender.send(&go_away).await.unwrap();
        let mut withdrawn = Vec::new();
        for _ in 0..2 {
            let event = next_event(&client).await;
            let SessionEvent::PublishNamespaceDone(done) = event else {
                panic!("expected PUBLISH_NAMESPACE_DONE, got {event:?}");
            };
            withdrawn.push(done.track_namespace().to_string());
        }
        let later = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            client.receive_event(),
        )
        .await;

        // Assert
        withdrawn.sort();
        assert_eq!(withdrawn, ["live/hall", "live/room"]);
        assert!(later.is_err(), "no duplicate withdrawal: {later:?}");
        assert!(
            ControlChannel::of(&client.publisher().session.connection())
                .requests()
                .outgoing
                .is_empty()
        );
    }

    fn outgoing_stream(session: &crate::Session, request_id: u64) -> super::RequestStream {
        ControlChannel::of(&session.publisher().session.connection())
            .requests()
            .outgoing
            .get(&request_id)
            .cloned()
            .unwrap()
    }

    fn request_update(request_id: u64, parameters: &[parameters::Parameter]) -> BytesMut {
        let mut payload = BytesMut::new();
        codec::put_vi64(&mut payload, request_id);
        parameters::put_parameters(&mut payload, parameters).unwrap();
        messages::frame(messages::REQUEST_UPDATE, &payload).unwrap()
    }

    #[tokio::test]
    async fn a_request_update_keeps_what_it_does_not_change() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-update", "moqt").await;
        let option = SubscribeOption {
            subscriber_priority: 7,
            filter_type: FilterType::AbsoluteStart {
                location: Location {
                    group_id: 5,
                    object_id: 0,
                },
            },
            ..SubscribeOption::default()
        };
        let subscription = subscribed(&client, &server, option).await;
        let stream = outgoing_stream(&client, subscription.request_id());
        let update = request_update(
            client
                .publisher()
                .session
                .connection()
                .control
                .next_request_id(),
            &[parameters::Parameter::Forward(false)],
        );

        // Act: an update that changes only FORWARD
        stream.sender.send(&update).await.unwrap();
        let event = next_event(&server).await;

        // Assert
        let SessionEvent::SubscribeUpdate(update) = event else {
            panic!("expected SUBSCRIBE_UPDATE, got {event:?}");
        };
        assert_eq!(update.subscription_request_id(), subscription.request_id());
        assert!(!update.forward());
        assert_eq!(update.subscriber_priority(), 7);
        assert_eq!(
            update.start_location(),
            Location {
                group_id: 5,
                object_id: 0,
            }
        );
    }

    /// A SUBSCRIBE `client` sends on a stream it reads itself, answered by
    /// `server`; returns the stream after its SUBSCRIBE_OK.
    async fn raw_subscription(
        client: &crate::Session,
        server: &crate::Session,
    ) -> (StreamSender, MessageReceiver) {
        let connection = client.publisher().session.connection();
        let (send_stream, receive_stream) = connection.transport.open_bi().await.unwrap();
        let sender = StreamSender::new(send_stream);
        let mut receiver = MessageReceiver::new(receive_stream, MessageDecoder);
        let subscribe = messages::encode_subscribe(&Subscribe {
            request_id: connection.control.next_request_id(),
            track_namespace: vec!["ns".to_string()],
            track_name: "track".to_string(),
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LargestObject,
            authorization_tokens: vec![],
            delivery_timeout: None,
        })
        .unwrap();
        sender.send(&subscribe).await.unwrap();
        let SessionEvent::Subscribe(handler) = next_event(server).await else {
            panic!("expected SUBSCRIBE");
        };
        handler.ok(0, ContentExists::False).await.unwrap();
        let response = receiver.receive().await.unwrap();
        assert!(
            matches!(response, Some(messages::Message::SubscribeOk(_))),
            "{response:?}"
        );
        (sender, receiver)
    }

    async fn requested_update(
        client: &crate::Session,
        server: &crate::Session,
        stream: &StreamSender,
    ) -> SubscribeUpdateHandler {
        let request_id = client
            .publisher()
            .session
            .connection()
            .control
            .next_request_id();
        stream
            .send(&request_update(
                request_id,
                &[parameters::Parameter::Forward(true)],
            ))
            .await
            .unwrap();
        let event = next_event(server).await;
        let SessionEvent::SubscribeUpdate(handler) = event else {
            panic!("expected SUBSCRIBE_UPDATE, got {event:?}");
        };
        handler
    }

    #[tokio::test]
    async fn a_request_update_is_answered_with_the_largest_object_the_publisher_gives() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-largest", "moqt").await;
        let (sender, mut receiver) = raw_subscription(&client, &server).await;
        let handler = requested_update(&client, &server, &sender).await;
        let largest = Location {
            group_id: 3,
            object_id: 1,
        };

        // Act
        let told = handler.ok(Some(largest));
        let response = receiver.receive().await.unwrap();

        // Assert: §10.2.11 LARGEST_OBJECT
        assert!(told);
        let Some(messages::Message::RequestOk(ok)) = response else {
            panic!("expected REQUEST_OK, got {response:?}");
        };
        assert_eq!(
            ok.parameters,
            vec![parameters::Parameter::LargestObject(largest)]
        );
    }

    #[tokio::test]
    async fn a_request_update_whose_handler_is_dropped_gets_a_request_ok_without_largest_object() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-dropped", "moqt").await;
        let (sender, mut receiver) = raw_subscription(&client, &server).await;
        let handler = requested_update(&client, &server, &sender).await;

        // Act
        drop(handler);
        let response = receiver.receive().await.unwrap();

        // Assert
        let Some(messages::Message::RequestOk(ok)) = response else {
            panic!("expected REQUEST_OK, got {response:?}");
        };
        assert!(ok.parameters.is_empty(), "{:?}", ok.parameters);
    }

    #[tokio::test]
    async fn the_answer_to_a_request_update_on_a_publish_keeps_the_subscription_open() {
        // Arrange: two updates; the publisher reads the second only after answering the first
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-publish-update", "moqt").await;
        let track = published_track_between(client, server).await;
        let subscriber = track.server.subscriber();
        subscriber
            .update_forward(&track.accepted, false)
            .await
            .unwrap();
        subscriber
            .update_forward(&track.accepted, true)
            .await
            .unwrap();
        let SessionEvent::SubscribeUpdate(first) = next_event(&track.client).await else {
            panic!("expected SUBSCRIBE_UPDATE");
        };
        first.ok(Some(Location {
            group_id: 3,
            object_id: 1,
        }));
        let SessionEvent::SubscribeUpdate(_second) = next_event(&track.client).await else {
            panic!("expected SUBSCRIBE_UPDATE");
        };

        // Act
        track
            .client
            .publisher()
            .publish_done(
                &track.published,
                PublishDoneCode::TrackEnded,
                0,
                String::new(),
            )
            .await
            .unwrap();
        let event = next_event(&track.server).await;

        // Assert: the subscriber read the REQUEST_OK before the PUBLISH_DONE
        assert!(matches!(event, SessionEvent::PublishDone(_)), "{event:?}");
    }

    #[tokio::test]
    async fn a_request_update_reusing_a_request_id_closes_the_session() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-request-id", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;
        let stream = outgoing_stream(&client, subscription.request_id());
        let update = request_update(subscription.request_id(), &[]);

        // Act
        stream.sender.send(&update).await.unwrap();
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed())
            .await
            .unwrap();

        // Assert: §10.1 INVALID_REQUEST_ID
        assert_eq!(close.code, Some(0x4), "{close:?}");
    }

    #[tokio::test]
    async fn a_request_update_on_a_track_status_closes_the_session() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-track-status-update", "moqt").await;
        let connection = client.publisher().session.connection();
        let (send_stream, _receive_stream) = connection.transport.open_bi().await.unwrap();
        let sender = StreamSender::new(send_stream);
        let track_status = messages::encode_track_status(&Subscribe {
            request_id: connection.control.next_request_id(),
            track_namespace: vec!["ns".to_string()],
            track_name: "track".to_string(),
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LargestObject,
            authorization_tokens: vec![],
            delivery_timeout: None,
        })
        .unwrap();
        sender.send(&track_status).await.unwrap();
        let SessionEvent::TrackStatus(_handler) = next_event(&server).await else {
            panic!("expected TRACK_STATUS");
        };

        // Act
        let update = request_update(connection.control.next_request_id(), &[]);
        sender.send(&update).await.unwrap();
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed())
            .await
            .unwrap();

        // Assert: §10.14, the subscriber cannot send REQUEST_UPDATE
        assert_eq!(close.code, Some(0x3), "{close:?}");
    }

    #[tokio::test]
    async fn a_lost_session_does_not_cancel_its_requests() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-lost", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;

        // Act
        drop((subscription, client));
        let event = tokio::time::timeout(DISCONNECT_TIMEOUT, server.receive_event()).await;

        // Assert: the session's end is reported, not as an UNSUBSCRIBE
        assert!(
            matches!(event, Ok(Ok(SessionEvent::Disconnected()))),
            "{event:?}"
        );
    }

    #[tokio::test]
    async fn a_publish_namespace_is_accepted() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-namespace", "moqt").await;
        let pending = client
            .publisher()
            .begin_publish_namespace("live/room".into())
            .await
            .unwrap();
        let SessionEvent::PublishNamespace(handler) = next_event(&server).await else {
            panic!("expected PUBLISH_NAMESPACE");
        };

        // Act
        handler.ok().await.unwrap();

        // Assert
        tokio::time::timeout(HANDSHAKE_TIMEOUT, pending.accepted())
            .await
            .expect("PUBLISH_NAMESPACE_OK should resolve the request")
            .unwrap();
    }
}

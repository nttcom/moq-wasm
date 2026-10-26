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
    /// Reaches the application as a namespace subscription for
    /// `NamespaceInterest::Tracks`.
    SubscribeTracks(Vec<String>),
}

impl RequestStreamKind {
    /// The prefix of a SUBSCRIBE_NAMESPACE `namespace` falls under; only that
    /// request takes NAMESPACE and NAMESPACE_DONE (§10.16, §10.17).
    fn namespace_prefix_of(&self, namespace: &[String]) -> Option<&[String]> {
        match self {
            Self::SubscribeNamespace(prefix) if namespace.starts_with(prefix) => Some(prefix),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct RequestStream {
    kind: RequestStreamKind,
    sender: Arc<StreamSender>,
    /// For a subscription: the priority its objects inherit when they omit
    /// one, from its SUBSCRIBE_OK or PUBLISH (§12.4).
    default_publisher_priority: u8,
    /// For a FETCH the peer sent: how far its response has come.
    fetch_end: FetchEnd,
    /// For a FETCH the peer sent: its response stream once opened, reset
    /// with the request stream when the FETCH is cancelled (§5.2).
    fetch_response: Option<Arc<StreamSender>>,
    /// Notified when this side cancels the request: the task reading the
    /// stream then stops the peer's half with STOP_SENDING (§3.3.2).
    reader_stop: Arc<tokio::sync::Notify>,
}

impl RequestStream {
    fn new(kind: RequestStreamKind, sender: Arc<StreamSender>) -> Self {
        Self {
            kind,
            sender,
            default_publisher_priority: messages::DEFAULT_PUBLISHER_PRIORITY,
            fetch_end: FetchEnd::default(),
            fetch_response: None,
            reader_stop: Arc::default(),
        }
    }
}

/// §5.2: a publisher destroys the state of a FETCH after closing its
/// response stream, so the request stream ends with FIN once the FETCH_OK
/// is sent too.
#[derive(Debug, Clone, Copy, Default)]
struct FetchEnd {
    fetch_ok_sent: bool,
    response_ended: bool,
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
    /// Held while a NAMESPACE or NAMESPACE_DONE is built and sent, and while
    /// a namespace subscription moves to a new prefix and is answered, so
    /// each suffix is relative to the prefix the subscriber last heard of
    /// (§10.9.2).
    namespace_writes: tokio::sync::Mutex<()>,
    binding: std::sync::OnceLock<Binding>,
}

fn cancelled() -> u64 {
    error_codes::stream_reset_code_to_wire(DataStreamResetCode::Cancelled)
}

/// Withdraws a request (§3.3.2): this side's half of its stream is reset,
/// and the peer's half stopped with STOP_SENDING, which a FETCH requires
/// (§5.2).
async fn cancel(stream: Option<RequestStream>) -> Result<(), TransportSendError> {
    match stream {
        Some(stream) => {
            stream.reader_stop.notify_one();
            stream.sender.reset(cancelled()).await
        }
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
            namespace_writes: tokio::sync::Mutex::new(()),
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

    /// The peer's SUBSCRIBE_NAMESPACE `namespace` falls under, with the
    /// length of its prefix.
    fn namespace_subscription(
        &self,
        namespace: &[String],
    ) -> Option<(RequestId, RequestStream, usize)> {
        self.requests()
            .incoming
            .iter()
            .find_map(|(request_id, stream)| {
                let prefix = stream.kind.namespace_prefix_of(namespace)?;
                Some((*request_id, stream.clone(), prefix.len()))
            })
    }

    /// draft-14 announces a namespace to a namespace subscriber with
    /// PUBLISH_NAMESPACE; draft-18 sends NAMESPACE on the subscription's
    /// stream instead (§6.1), once: a namespace the subscription already
    /// heard of is not sent again. The announcement is accepted at once, as
    /// no response follows. `None` when the peer has no SUBSCRIBE_NAMESPACE
    /// for it, so it goes as PUBLISH_NAMESPACE.
    async fn advertise_namespace(
        &self,
        request_id: RequestId,
        namespace: &[String],
    ) -> Result<Option<PendingResponse>, RequestFailure> {
        let _namespace_writes = self.namespace_writes.lock().await;
        let Some((subscription_request_id, stream, prefix_len)) =
            self.namespace_subscription(namespace)
        else {
            return Ok(None);
        };
        let accepted = PendingResponse::resolved(ResponseMessage::PublishNamespaceOk(request_id));
        if self.requests().advertised.get(namespace) == Some(&subscription_request_id) {
            return Ok(Some(accepted));
        }
        let message = messages::encode_namespace(messages::NAMESPACE, &namespace[prefix_len..])
            .map_err(|_| RequestFailure::ProtocolViolation("namespace could not be encoded"))?;
        stream.sender.send(&message).await?;
        self.requests()
            .advertised
            .insert(namespace.to_vec(), subscription_request_id);
        Ok(Some(accepted))
    }

    /// §10.9.2: the peer's SUBSCRIBE_NAMESPACE or SUBSCRIBE_TRACKS
    /// `request_id`, of `kind`, moves to `prefix` and is answered with
    /// REQUEST_OK on `sender`. A namespace announced under the old prefix that
    /// the new one leaves out is forgotten, as no NAMESPACE_DONE could name it.
    /// Returns the subscription's new kind.
    async fn move_namespace_subscription(
        &self,
        request_id: RequestId,
        kind: &RequestStreamKind,
        prefix: Vec<String>,
        sender: &StreamSender,
    ) -> RequestStreamKind {
        let moved = match kind {
            RequestStreamKind::SubscribeNamespace(_) => {
                RequestStreamKind::SubscribeNamespace(prefix)
            }
            RequestStreamKind::SubscribeTracks(_) => RequestStreamKind::SubscribeTracks(prefix),
            _ => unreachable!("only namespace subscriptions take a TRACK_NAMESPACE_PREFIX"),
        };
        let _namespace_writes = self.namespace_writes.lock().await;
        {
            let mut requests = self.requests();
            if let Some(stream) = requests.incoming.get_mut(&request_id) {
                stream.kind = moved.clone();
            }
            requests.advertised.retain(|namespace, subscription| {
                *subscription != request_id || moved.namespace_prefix_of(namespace).is_some()
            });
        }
        if let Ok(message) = messages::encode_empty_request_ok()
            && let Err(error) = sender.send(&message).await
        {
            tracing::debug!(%error, "could not answer REQUEST_UPDATE");
        }
        moved
    }

    pub(super) fn default_publisher_priority(&self, request_id: RequestId) -> u8 {
        let requests = self.requests();
        requests
            .outgoing
            .get(&request_id)
            .or_else(|| requests.incoming.get(&request_id))
            .map_or(messages::DEFAULT_PUBLISHER_PRIORITY, |stream| {
                stream.default_publisher_priority
            })
    }

    fn set_default_publisher_priority(&self, request_id: RequestId, priority: u8) {
        if let Some(stream) = self.requests().outgoing.get_mut(&request_id) {
            stream.default_publisher_priority = priority;
        }
    }

    /// Records `progress` on the FETCH the peer sent as `request_id`, and
    /// ends its request stream with FIN once both FETCH_OK and the end of
    /// the response stream are done.
    async fn advance_fetch(
        &self,
        request_id: RequestId,
        progress: impl FnOnce(&mut FetchEnd),
    ) -> Result<(), TransportSendError> {
        let ended = {
            let mut requests = self.requests();
            let Some(stream) = requests.incoming.get_mut(&request_id) else {
                return Ok(());
            };
            progress(&mut stream.fetch_end);
            if !(stream.fetch_end.fetch_ok_sent && stream.fetch_end.response_ended) {
                return Ok(());
            }
            requests.incoming.remove(&request_id)
        };
        match ended {
            Some(stream) => stream.sender.close().await,
            None => Ok(()),
        }
    }

    pub(super) async fn fetch_response_stopped(
        connection: &Arc<Connection>,
        request_id: RequestId,
    ) {
        if let Some(scope) = Scope::of(&Arc::downgrade(connection)) {
            request_streams::fetch_cancelled(&scope, request_id).await;
        }
    }

    pub(super) async fn fetch_response_ended(
        &self,
        request_id: RequestId,
    ) -> Result<(), TransportSendError> {
        self.advance_fetch(request_id, |end| end.response_ended = true)
            .await
    }

    pub(super) fn attach_fetch_response(
        &self,
        request_id: RequestId,
        response: &Arc<StreamSender>,
    ) -> bool {
        match self.requests().incoming.get_mut(&request_id) {
            Some(stream) if stream.kind == RequestStreamKind::Fetch => {
                stream.fetch_response = Some(response.clone());
                true
            }
            _ => false,
        }
    }

    /// §5.2: the publisher of a cancelled FETCH resets its request stream
    /// and its response stream. `false` when the FETCH the peer sent as
    /// `request_id` had ended already.
    async fn reset_cancelled_fetch(&self, request_id: RequestId) -> bool {
        let Some(stream) = self.take_incoming(request_id) else {
            return false;
        };
        let response = stream.fetch_response.clone();
        if let Err(error) = cancel(Some(stream)).await {
            tracing::debug!(%error, "could not reset the request stream of a cancelled FETCH");
        }
        if let Some(response) = response
            && let Err(error) = response.reset(cancelled()).await
        {
            tracing::debug!(%error, "could not reset the response stream of a cancelled FETCH");
        }
        true
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
            OutgoingRequest::SubscribeTracks(subscribe_tracks) => (
                subscribe_tracks.request_id,
                RequestStreamKind::SubscribeTracks(subscribe_tracks.track_namespace_prefix.clone()),
                messages::encode_subscribe_tracks(&messages::SubscribeTracksMessage {
                    request_id: subscribe_tracks.request_id,
                    track_namespace_prefix: subscribe_tracks.track_namespace_prefix,
                    authorization_tokens: subscribe_tracks.authorization_token,
                    forward: true,
                })
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
        let stream = RequestStream::new(kind, sender);
        self.requests().outgoing.insert(request_id, stream.clone());
        let (response_sender, receiver) = tokio::sync::oneshot::channel();
        request_streams::OutgoingRequestTask {
            connection: Arc::downgrade(connection),
            request_id,
            stream,
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
                self.send_on_incoming(ok.request_id, message, false).await?;
                self.advance_fetch(ok.request_id, |end| end.fetch_ok_sent = true)
                    .await
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
                let _namespace_writes = self.namespace_writes.lock().await;
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
            Notification::UnsubscribeNamespace {
                track_namespace_prefix,
                interest,
            } => {
                let mut kinds = Vec::new();
                if interest.includes_namespaces() {
                    kinds.push(RequestStreamKind::SubscribeNamespace(
                        track_namespace_prefix.clone(),
                    ));
                }
                if interest.includes_tracks() {
                    kinds.push(RequestStreamKind::SubscribeTracks(track_namespace_prefix));
                }
                let streams: Vec<RequestStream> = {
                    let mut requests = self.requests();
                    kinds
                        .iter()
                        .filter_map(|kind| Self::take_by_kind(&mut requests.outgoing, kind))
                        .collect()
                };
                for stream in streams {
                    cancel(Some(stream)).await?;
                }
                Ok(())
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

/// §5.2: a subscriber keeps the state of a FETCH until its response stream
/// ends with FIN or a reset. Ends the request stream of a FETCH this side
/// sent with FIN then, or cancels it (`cancel`) when the reader of the
/// response stream is dropped first.
#[derive(Debug)]
pub(crate) struct FetchRequestEnd {
    connection: Weak<Connection>,
    request_id: RequestId,
    response_ended: bool,
}

impl FetchRequestEnd {
    pub(super) fn new(connection: Weak<Connection>, request_id: RequestId) -> Self {
        Self {
            connection,
            request_id,
            response_ended: false,
        }
    }

    pub(crate) fn response_ended(mut self) {
        self.response_ended = true;
    }
}

impl Drop for FetchRequestEnd {
    fn drop(&mut self) {
        end_outgoing(&self.connection, self.request_id, self.response_ended);
    }
}

/// Ends the stream of the request this side sent as `request_id`, if it is
/// still open: with FIN when `finished`, otherwise with a reset that cancels
/// the request (§3.3.2). Called from `Drop`, so it does not wait and, outside
/// a runtime, leaves the stream to its own drop.
fn end_outgoing(connection: &Weak<Connection>, request_id: RequestId, finished: bool) {
    let Some(connection) = connection.upgrade() else {
        return;
    };
    let Some(stream) = ControlChannel::of(&connection).take_outgoing(request_id) else {
        return;
    };
    executor::try_spawn("end draft-18 request", async move {
        let ended = if finished {
            stream.sender.close().await
        } else {
            cancel(Some(stream)).await
        };
        if let Err(error) = ended {
            tracing::debug!(%error, "could not end a request stream");
        }
    });
}

pub(crate) struct PendingResponse {
    receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    /// The request whose stream is reset when the caller stops waiting
    /// before a success response; that withdraws the request (§3.3.2).
    cancel: Option<(Weak<Connection>, RequestId)>,
}

impl Drop for PendingResponse {
    fn drop(&mut self) {
        if let Some((connection, request_id)) = self.cancel.take() {
            end_outgoing(&connection, request_id, false);
        }
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
    use std::sync::Arc;

    use bytes::{Bytes, BytesMut};

    use super::{
        ControlChannel, Scope, codec,
        message_stream::{MessageDecoder, MessageReceiver},
        messages, parameters,
        request_streams::OutgoingRequestTask,
    };

    use crate::{
        RequestErrorCode, RequestFailure, SessionEvent, TrackWriter,
        fetch::{
            Fetch, FetchDataReceiver, FetchDataSender, FetchHandler, FetchOption, FetchParams,
            UnknownRange,
        },
        modules::{
            moqt::{
                control_plane::{
                    control_messages::messages::{
                        fetch::Fetch as FetchMessage, parameters::group_order::GroupOrder,
                        subscribe::Subscribe, subscribe_ok::SubscribeOk,
                    },
                    enums::ResponseMessage,
                },
                domains::connection::IncomingObjectNotification,
            },
            moqt::{data_plane::stream::stream_sender::StreamSender, version::Version},
            test_support::{
                DISCONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, accept_publish, data_receiver,
                datagram_object, mandatory_track_properties, next_datagram, next_subgroup,
                notify_datagram, published_track_between, read_fetch_objects,
                spawn_connected_draft18_sessions, subscribed_track_reader,
            },
            transport::transport_send_stream::TransportSendError,
        },
        namespace::{NamespaceInterest, SubscribeNamespaceHandler},
        object::{
            ContentExists, DataReceiver, ForwardingPreference, KeyValuePair, Location, Object,
            ObjectStatus, Subgroup, VariantType,
        },
        publish::{PublishDoneCode, PublishOption},
        subscribe::{
            FilterType, SubscribeHandler, SubscribeOption, SubscribeUpdateHandler, UpdateAnswer,
        },
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
                track_properties: Default::default(),
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
        let object = next_datagram(&mut data_receiver(&track.server, &track.accepted).await).await;

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

    /// More FETCHes than the 1000 concurrent bidirectional streams a
    /// connection allows.
    const SEQUENTIAL_FETCHES: usize = 1010;

    #[tokio::test]
    async fn completed_fetches_release_their_request_streams() {
        for scheme in SCHEMES {
            // Arrange
            let (client, server) = spawn_connected_draft18_sessions(
                &format!("draft18-fetch-release-{scheme}"),
                scheme,
            )
            .await;
            let end = Location {
                group_id: 0,
                object_id: 1,
            };
            let object = Object {
                location: Location {
                    group_id: 0,
                    object_id: 0,
                },
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
                publisher_priority: 128,
                status: ObjectStatus::Normal,
                extension_headers: Default::default(),
                payload: Bytes::from_static(b"o"),
                ends_group: false,
            };
            let answering = tokio::spawn(async move {
                for _ in 0..SEQUENTIAL_FETCHES {
                    let SessionEvent::Fetch(handler) = next_event(&server).await else {
                        panic!("expected FETCH");
                    };
                    handler.ok(false, end).await.unwrap();
                    let sender = server
                        .publisher()
                        .create_fetch_stream(handler.request_id)
                        .await
                        .unwrap();
                    sender.send_object(&object).await.unwrap();
                    sender.close().await.unwrap();
                }
                let SessionEvent::Subscribe(handler) = next_event(&server).await else {
                    panic!("expected SUBSCRIBE");
                };
                handler.ok(0, ContentExists::False).await.unwrap();
                server
            });
            let mut subscriber = client.subscriber();

            // Act
            for index in 0..SEQUENTIAL_FETCHES {
                let handle = tokio::time::timeout(
                    HANDSHAKE_TIMEOUT,
                    subscriber.fetch(
                        "ns".into(),
                        "track".into(),
                        Location {
                            group_id: 0,
                            object_id: 0,
                        },
                        end,
                        FetchOption::default(),
                    ),
                )
                .await
                .unwrap_or_else(|_| panic!("{scheme}: FETCH {index} was not answered"))
                .unwrap();
                let mut receiver = subscriber.accept_fetch_receiver(&handle).await.unwrap();
                assert_eq!(read_fetch_objects(&mut receiver).await.len(), 1);
            }
            let subscription = tokio::time::timeout(
                HANDSHAKE_TIMEOUT,
                subscriber.subscribe("ns".into(), "track".into(), SubscribeOption::default()),
            )
            .await;

            // Assert: only the subscription keeps a request stream
            assert!(matches!(subscription, Ok(Ok(_))), "{scheme}");
            let server = answering.await.unwrap();
            assert_eq!(
                ControlChannel::of(&client.publisher().session.connection())
                    .requests()
                    .outgoing
                    .len(),
                1,
                "{scheme}"
            );
            assert_eq!(
                ControlChannel::of(&server.publisher().session.connection())
                    .requests()
                    .incoming
                    .len(),
                1,
                "{scheme}"
            );
        }
    }

    /// A FETCH from `client` that `server` answers with FETCH_OK and a
    /// response stream it keeps open.
    async fn fetch_with_open_response(
        client: &crate::Session,
        server: &crate::Session,
    ) -> (
        crate::Subscriber,
        FetchDataReceiver,
        FetchHandler,
        FetchDataSender,
    ) {
        let end = Location {
            group_id: 1,
            object_id: 0,
        };
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            let handle = subscriber
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
                .unwrap();
            let receiver = subscriber.accept_fetch_receiver(&handle).await.unwrap();
            (subscriber, receiver)
        });
        let SessionEvent::Fetch(handler) = next_event(server).await else {
            panic!("expected FETCH");
        };
        handler.ok(false, end).await.unwrap();
        let sender = server
            .publisher()
            .create_fetch_stream(handler.request_id)
            .await
            .unwrap();
        let (subscriber, receiver) = request.await.unwrap();
        (subscriber, receiver, handler, sender)
    }

    #[tokio::test]
    async fn a_fetch_response_reports_an_unknown_range_and_goes_on() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-fetch-unknown-range", "moqt").await;
        let (_subscriber, mut receiver, _handler, sender) =
            fetch_with_open_response(&client, &server).await;
        let object = |group_id| Object {
            location: Location {
                group_id,
                object_id: 0,
            },
            forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
            publisher_priority: 128,
            status: ObjectStatus::Normal,
            extension_headers: Default::default(),
            payload: Bytes::from_static(b"o"),
            ends_group: false,
        };
        let unknown_through = Location {
            group_id: 2,
            object_id: u64::MAX,
        };

        // Act
        sender.send_object(&object(0)).await.unwrap();
        let reported = sender.report_unknown_range(unknown_through).await.unwrap();
        sender.send_object(&object(3)).await.unwrap();
        sender.close().await.unwrap();
        let mut received = Vec::new();
        loop {
            let fetched = receiver.receive().await.unwrap();
            let end = fetched == Fetch::End;
            received.push(fetched);
            if end {
                break;
            }
        }

        // Assert: §11.4.4.2
        assert_eq!(reported, UnknownRange::Marked);
        assert_eq!(
            received,
            vec![
                Fetch::Object(object(0)),
                Fetch::UnknownRange {
                    last: unknown_through
                },
                Fetch::Object(object(3)),
                Fetch::End,
            ]
        );
    }

    #[tokio::test]
    async fn dropping_a_fetch_receiver_before_the_end_cancels_the_fetch() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-fetch-receiver-dropped", "moqt").await;
        let (_subscriber, receiver, _handler, _sender) =
            fetch_with_open_response(&client, &server).await;

        // Act
        drop(receiver);
        let event = next_event(&server).await;

        // Assert: §3.3.2
        assert!(matches!(event, SessionEvent::FetchCancel(_)), "{event:?}");
    }

    #[tokio::test]
    async fn cancelling_a_fetch_stops_its_request_stream() {
        for scheme in SCHEMES {
            // Arrange
            let (client, server) =
                spawn_connected_draft18_sessions(&format!("draft18-fetch-stop-{scheme}"), scheme)
                    .await;
            let (_subscriber, receiver, handler, _sender) =
                fetch_with_open_response(&client, &server).await;
            // Arrange: taken out of the registry, so the publisher leaves the stream to the test
            let request_stream = ControlChannel::of(&server.publisher().session.connection())
                .requests()
                .incoming
                .remove(&handler.request_id)
                .unwrap();
            let request_ok = messages::encode_empty_request_ok().unwrap();

            // Act
            drop(receiver);
            let refused = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
                loop {
                    if let Err(error) = request_stream.sender.send(&request_ok).await {
                        return error;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;

            // Assert: §5.2 STOP_SENDING with CANCELLED (§3.3.3)
            assert!(
                matches!(refused, Ok(TransportSendError::Stopped { code: 0x1 })),
                "{scheme}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_fetch_receiver_dropped_outside_a_runtime_does_not_panic() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-fetch-receiver-off-runtime", "moqt").await;
        let (_subscriber, receiver, _handler, _sender) =
            fetch_with_open_response(&client, &server).await;

        // Act
        let dropped = std::thread::spawn(move || drop(receiver)).join();

        // Assert
        assert!(dropped.is_ok());
    }

    #[tokio::test]
    async fn a_fetch_sender_dropped_unended_releases_the_request_stream() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-fetch-sender-dropped", "moqt").await;
        let (_subscriber, mut receiver, _handler, sender) =
            fetch_with_open_response(&client, &server).await;

        // Act
        drop(sender);
        let received = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.receive()).await;
        let released = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            while !ControlChannel::of(&server.publisher().session.connection())
                .requests()
                .incoming
                .is_empty()
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;

        // Assert: an unfinished response is reset, not ended with FIN (§5.2)
        assert!(matches!(received, Ok(Err(_))), "{received:?}");
        assert!(released.is_ok());
    }

    #[tokio::test]
    async fn a_fetch_stream_for_a_subscribe_leaves_the_subscribe_open() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-fetch-stream-for-subscribe", "moqt").await;
        let (_subscription, handler) =
            subscribed_with_track_properties(&client, &server, 4, &[]).await;

        // Act
        send_raw_stream(
            &server,
            &Version::Draft18.encode_fetch_header(handler.request_id()),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Assert: only a FETCH's own response stream ends its request
        assert!(
            ControlChannel::of(&client.publisher().session.connection())
                .requests()
                .outgoing
                .contains_key(&handler.request_id())
        );
    }

    #[tokio::test]
    async fn a_stop_sending_after_the_requester_fin_on_an_ended_fetch_ends_its_reading() {
        // Arrange: taken out of the registry, as a FETCH that has ended is
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-fetch-ended-stop", "moqt").await;
        let (_subscriber, receiver, handler, _sender) =
            fetch_with_open_response(&client, &server).await;
        let request_stream = ControlChannel::of(&server.publisher().session.connection())
            .requests()
            .incoming
            .remove(&handler.request_id)
            .unwrap();
        let client_request_stream = outgoing_stream(&client, receiver.request_id);

        // Act: FIN, then STOP_SENDING on the publisher's half
        client_request_stream.sender.close().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        client_request_stream.reader_stop.notify_one();
        let released = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
            while Arc::strong_count(&request_stream.sender) > 1 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;

        // Assert: the task reading the request stream ends
        assert!(released.is_ok());
    }

    #[tokio::test]
    async fn a_stop_sending_on_a_fetch_request_stream_resets_both_its_streams() {
        for scheme in SCHEMES {
            // Arrange
            let (client, server) = spawn_connected_draft18_sessions(
                &format!("draft18-fetch-stopped-{scheme}"),
                scheme,
            )
            .await;
            let (_subscriber, mut receiver, handler, _sender) =
                fetch_with_open_response(&client, &server).await;
            let request_stream = ControlChannel::of(&server.publisher().session.connection())
                .requests()
                .incoming
                .get(&handler.request_id)
                .cloned()
                .unwrap();

            // Act: STOP_SENDING alone, the requester's half left open
            outgoing_stream(&client, receiver.request_id)
                .reader_stop
                .notify_one();
            let event = next_event(&server).await;
            let response = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.receive()).await;
            let request_reply = request_stream
                .sender
                .send(&messages::encode_empty_request_ok().unwrap())
                .await;

            // Assert: §5.2 the publisher resets the request and response streams
            assert_fetch_cancelled_with_reset_request_stream(scheme, event, request_reply);
            assert!(
                matches!(&response, Ok(Err(error)) if error.to_string().contains("reset")),
                "{scheme}: {response:?}"
            );
        }
    }

    /// A FETCH whose requester has sent STOP_SENDING on the response stream
    /// alone; returns the publisher's data sender and its request stream.
    async fn fetch_with_stopped_response_stream(
        client: &crate::Session,
        server: &crate::Session,
    ) -> (FetchDataSender, super::RequestStream) {
        let (_subscriber, receiver, handler, sender) =
            fetch_with_open_response(client, server).await;
        // Taken out of the registry, so dropping the receiver leaves the request stream alone.
        ControlChannel::of(&client.publisher().session.connection())
            .requests()
            .outgoing
            .remove(&receiver.request_id)
            .unwrap();
        let request_stream = ControlChannel::of(&server.publisher().session.connection())
            .requests()
            .incoming
            .get(&handler.request_id)
            .cloned()
            .unwrap();
        drop(receiver);
        let response_stream = request_stream.fetch_response.clone().unwrap();
        let stopped = tokio::time::timeout(HANDSHAKE_TIMEOUT, response_stream.stopped()).await;
        assert!(matches!(stopped, Ok(Some(_))), "{stopped:?}");
        (sender, request_stream)
    }

    fn assert_fetch_cancelled_with_reset_request_stream(
        scheme: &str,
        event: SessionEvent,
        request_reply: Result<(), TransportSendError>,
    ) {
        assert!(
            matches!(event, SessionEvent::FetchCancel(_)),
            "{scheme}: {event:?}"
        );
        assert!(
            matches!(request_reply, Err(TransportSendError::ClosedStream)),
            "{scheme}: {request_reply:?}"
        );
    }

    #[tokio::test]
    async fn an_object_sent_on_a_stopped_fetch_response_resets_both_streams() {
        for scheme in SCHEMES {
            // Arrange
            let (client, server) = spawn_connected_draft18_sessions(
                &format!("draft18-fetch-response-stopped-send-{scheme}"),
                scheme,
            )
            .await;
            let (sender, request_stream) =
                fetch_with_stopped_response_stream(&client, &server).await;
            let object = Object {
                location: Location {
                    group_id: 0,
                    object_id: 0,
                },
                forwarding: ForwardingPreference::Subgroup { subgroup_id: 0 },
                publisher_priority: 128,
                status: ObjectStatus::Normal,
                extension_headers: Default::default(),
                payload: Bytes::from_static(b"o"),
                ends_group: false,
            };

            // Act
            let sent = sender.send_object(&object).await;
            let event = next_event(&server).await;
            let request_reply = request_stream
                .sender
                .send(&messages::encode_empty_request_ok().unwrap())
                .await;

            // Assert: §5.2
            assert!(sent.is_err(), "{scheme}: {sent:?}");
            assert_fetch_cancelled_with_reset_request_stream(scheme, event, request_reply);
        }
    }

    #[tokio::test]
    async fn closing_a_stopped_fetch_response_resets_both_streams() {
        for scheme in SCHEMES {
            // Arrange
            let (client, server) = spawn_connected_draft18_sessions(
                &format!("draft18-fetch-response-stopped-close-{scheme}"),
                scheme,
            )
            .await;
            let (sender, request_stream) =
                fetch_with_stopped_response_stream(&client, &server).await;

            // Act
            let closed = sender.close().await;
            let event = next_event(&server).await;
            let request_reply = request_stream
                .sender
                .send(&messages::encode_empty_request_ok().unwrap())
                .await;

            // Assert: §5.2
            assert!(closed.is_err(), "{scheme}: {closed:?}");
            assert_fetch_cancelled_with_reset_request_stream(scheme, event, request_reply);
        }
    }

    #[tokio::test]
    async fn a_fin_after_a_fetch_does_not_cancel_it() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-fetch-fin", "moqt").await;
        let connection = client.publisher().session.connection();
        let (send_stream, _receive_stream) = connection.transport.open_bi().await.unwrap();
        let sender = StreamSender::new(send_stream);
        let fetch = messages::encode_fetch(&FetchMessage {
            request_id: connection.control.next_request_id(),
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            fetch_params: FetchParams::Standalone {
                track_namespace: vec!["ns".to_string()],
                track_name: "track".to_string(),
                start_location: Location {
                    group_id: 0,
                    object_id: 0,
                },
                end_location: Location {
                    group_id: 1,
                    object_id: 0,
                },
            },
            authorization_tokens: vec![],
            fill_timeout: None,
        })
        .unwrap();
        sender.send(&fetch).await.unwrap();

        // Act
        sender.close().await.unwrap();
        let SessionEvent::Fetch(handler) = next_event(&server).await else {
            panic!("expected FETCH");
        };
        let later = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            server.receive_event(),
        )
        .await;
        let answered = handler
            .ok(
                false,
                Location {
                    group_id: 1,
                    object_id: 0,
                },
            )
            .await;

        // Assert: §3.3.2 cancels by reset, not by FIN
        assert!(later.is_err(), "{later:?}");
        assert!(answered.is_ok(), "{answered:?}");
    }

    #[tokio::test]
    async fn a_draft18_fetch_hands_its_fill_timeout_to_the_handler() {
        // Arrange: a FETCH with FILL_TIMEOUT 0, which no FetchOption sets
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-fetch-fill-timeout", "moqt").await;
        let connection = client.publisher().session.connection();
        let (send_stream, _receive_stream) = connection.transport.open_bi().await.unwrap();
        let fetch = messages::encode_fetch(&FetchMessage {
            request_id: connection.control.next_request_id(),
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            fetch_params: FetchParams::Standalone {
                track_namespace: vec!["ns".to_string()],
                track_name: "track".to_string(),
                start_location: Location {
                    group_id: 0,
                    object_id: 0,
                },
                end_location: Location {
                    group_id: 1,
                    object_id: 0,
                },
            },
            authorization_tokens: vec![],
            fill_timeout: Some(0),
        })
        .unwrap();

        // Act
        StreamSender::new(send_stream).send(&fetch).await.unwrap();
        let SessionEvent::Fetch(handler) = next_event(&server).await else {
            panic!("expected FETCH");
        };

        // Assert: §10.2.5
        assert_eq!(handler.fill_timeout(), Some(std::time::Duration::ZERO));
    }

    #[tokio::test]
    async fn a_namespace_subscriber_hears_namespaces_as_publish_namespace() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-namespaces", "moqt").await;
        let subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe_namespace("live".into(), NamespaceInterest::Namespaces)
                .await
        });
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
        forward: bool,
    ) -> (StreamSender, MessageReceiver, SubscribeNamespaceHandler) {
        let connection = client.publisher().session.connection();
        let (send_stream, receive_stream) = connection.transport.open_bi().await.unwrap();
        let sender = StreamSender::new(send_stream);
        let message = messages::encode_subscribe_tracks(&messages::SubscribeTracksMessage {
            request_id: connection.control.next_request_id(),
            track_namespace_prefix: vec![prefix.to_string()],
            authorization_tokens: vec![],
            forward,
        })
        .unwrap();
        sender.send(&message).await.unwrap();
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
    async fn subscribe_tracks_reaches_the_publisher_as_a_tracks_interest_with_its_forward() {
        for forward in [false, true] {
            // Arrange
            let (client, server) = spawn_connected_draft18_sessions("draft18-tracks", "moqt").await;

            // Act
            let (_sender, mut receiver, handler) =
                subscribe_tracks_requested(&client, &server, "live", forward).await;
            handler.ok().await.unwrap();
            let response = receiver.receive().await.unwrap();

            // Assert: §6.1 leaves out the subscriber's own tracks
            assert_eq!(handler.track_namespace_prefix, "live");
            assert_eq!(handler.interest, NamespaceInterest::Tracks);
            assert_eq!(handler.forward, forward);
            assert!(
                matches!(response, Some(messages::Message::RequestOk(_))),
                "{response:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_tracks_subscriber_hears_a_namespace_as_publish_namespace() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-tracks-namespace", "moqt").await;
        let (_sender, mut receiver, handler) =
            subscribe_tracks_requested(&client, &server, "live", true).await;
        handler.ok().await.unwrap();
        receiver.receive().await.unwrap();

        // Act
        let publisher = server.publisher();
        let announcing =
            tokio::spawn(async move { publisher.publish_namespace("live/room".into()).await });
        let announced = next_event(&client).await;

        // Assert: a SUBSCRIBE_TRACKS takes no NAMESPACE (§10.16), so the
        // namespace goes as its own request
        let SessionEvent::PublishNamespace(announced) = announced else {
            panic!("expected PUBLISH_NAMESPACE, got {announced:?}");
        };
        assert_eq!(announced.track_namespace, "live/room");
        announced.ok().await.unwrap();
        assert!(announcing.await.unwrap().is_ok());
    }

    /// Calls `subscribe_namespace("live", interest)` on `client`; returns
    /// the pending call and the `count` handlers `server` receives.
    async fn namespace_requests(
        client: &crate::Session,
        server: &crate::Session,
        interest: NamespaceInterest,
        count: usize,
    ) -> (
        tokio::task::JoinHandle<Result<(), RequestFailure>>,
        Vec<SubscribeNamespaceHandler>,
    ) {
        let subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe_namespace("live".into(), interest)
                .await
        });
        let mut handlers = Vec::new();
        for _ in 0..count {
            let event = next_event(server).await;
            let SessionEvent::SubscribeNameSpace(handler) = event else {
                panic!("expected SUBSCRIBE_NAMESPACE, got {event:?}");
            };
            handlers.push(handler);
        }
        (request, handlers)
    }

    fn interests<'a>(
        interests: impl IntoIterator<Item = &'a NamespaceInterest>,
    ) -> std::collections::HashSet<NamespaceInterest> {
        interests.into_iter().copied().collect()
    }

    #[tokio::test]
    async fn namespaces_and_tracks_sends_subscribe_namespace_and_subscribe_tracks() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespaces-and-tracks-requests", "moqt")
                .await;

        // Act
        let (request, handlers) =
            namespace_requests(&client, &server, NamespaceInterest::NamespacesAndTracks, 2).await;
        for handler in &handlers {
            handler.ok().await.unwrap();
        }

        // Assert
        assert_eq!(
            interests(handlers.iter().map(|handler| &handler.interest)),
            interests(&[NamespaceInterest::Namespaces, NamespaceInterest::Tracks])
        );
        assert!(request.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn a_rejected_subscribe_tracks_withdraws_the_accepted_subscribe_namespace() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespaces-and-tracks-rejected", "moqt")
                .await;
        let (request, handlers) =
            namespace_requests(&client, &server, NamespaceInterest::NamespacesAndTracks, 2).await;

        // Act
        for handler in &handlers {
            if handler.interest == NamespaceInterest::Tracks {
                handler
                    .error(RequestErrorCode::NamespacePrefixOverlap, String::new())
                    .await
                    .unwrap();
            } else {
                handler.ok().await.unwrap();
            }
        }
        let subscribed = request.await.unwrap();
        let withdrawn = next_event(&server).await;

        // Assert
        assert!(
            matches!(
                subscribed,
                Err(RequestFailure::Rejected {
                    code: RequestErrorCode::NamespacePrefixOverlap,
                    ..
                })
            ),
            "{subscribed:?}"
        );
        let SessionEvent::UnsubscribeNamespace(withdrawn) = withdrawn else {
            panic!("expected the SUBSCRIBE_NAMESPACE withdrawn, got {withdrawn:?}");
        };
        assert_eq!(withdrawn.interest, NamespaceInterest::Namespaces);
        assert_eq!(withdrawn.track_namespace_prefix(), "live");
    }

    #[tokio::test]
    async fn unsubscribe_namespace_withdraws_subscribe_namespace_and_subscribe_tracks() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespaces-and-tracks-withdrawn", "moqt")
                .await;
        let (request, handlers) =
            namespace_requests(&client, &server, NamespaceInterest::NamespacesAndTracks, 2).await;
        for handler in &handlers {
            handler.ok().await.unwrap();
        }
        request.await.unwrap().unwrap();

        // Act
        client
            .subscriber()
            .unsubscribe_namespace("live".into())
            .await
            .unwrap();
        let mut withdrawn = Vec::new();
        for _ in 0..2 {
            let event = next_event(&server).await;
            let SessionEvent::UnsubscribeNamespace(handler) = event else {
                panic!("expected a namespace subscription withdrawn, got {event:?}");
            };
            withdrawn.push(handler.interest);
        }

        // Assert
        assert_eq!(
            interests(&withdrawn),
            interests(&[NamespaceInterest::Namespaces, NamespaceInterest::Tracks])
        );
    }

    #[tokio::test]
    async fn publish_blocked_on_a_subscribe_tracks_stream_keeps_the_session() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-publish-blocked", "moqt").await;
        let (request, handlers) =
            namespace_requests(&client, &server, NamespaceInterest::Tracks, 1).await;
        handlers[0].ok().await.unwrap();
        request.await.unwrap().unwrap();
        let stream = ControlChannel::of(&server.publisher().session.connection())
            .requests()
            .incoming
            .values()
            .find(|stream| {
                stream.kind == super::RequestStreamKind::SubscribeTracks(vec!["live".into()])
            })
            .cloned()
            .unwrap();
        let mut payload = BytesMut::new();
        codec::put_track_namespace(&mut payload, &["room".to_string()]);
        codec::put_vi64(&mut payload, 5);
        payload.extend_from_slice(b"video");

        // Act
        stream
            .sender
            .send(&messages::frame(messages::PUBLISH_BLOCKED, &payload).unwrap())
            .await
            .unwrap();
        let (request, handlers) =
            namespace_requests(&client, &server, NamespaceInterest::Namespaces, 1).await;
        handlers[0].ok().await.unwrap();

        // Assert: the session survives the PUBLISH_BLOCKED (§6.1), so a later request is answered
        assert!(request.await.unwrap().is_ok());
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
                subscribe_tracks_requested(&client, &server, "live", true).await;
            handler.ok().await.unwrap();
            tracks_streams.push((sender, receiver));
        }
        let subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe_namespace("live".into(), NamespaceInterest::Namespaces)
                .await
        });
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
    async fn a_response_of_another_request_kind_closes_the_session_with_a_protocol_violation() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-wrong-response-kind", "moqt").await;
        let (request, handler) =
            subscribe_requested(&client, &server, SubscribeOption::default()).await;
        let stream = ControlChannel::of(&server.publisher().session.connection())
            .requests()
            .incoming
            .get(&handler.request_id())
            .cloned()
            .unwrap();

        // Act: a SUBSCRIBE is answered by SUBSCRIBE_OK, not REQUEST_OK (§10.5)
        stream
            .sender
            .send(&messages::encode_empty_request_ok().unwrap())
            .await
            .unwrap();
        let closed = tokio::time::timeout(DISCONNECT_TIMEOUT, server.closed()).await;
        let subscribed = tokio::time::timeout(DISCONNECT_TIMEOUT, request).await;

        // Assert
        assert_eq!(
            closed.expect("the client closes the session").code,
            Some(0x3)
        );
        assert!(subscribed.unwrap().unwrap().is_err());
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

    fn request_go_away(new_session_uri: &str) -> BytesMut {
        messages::encode_go_away(&messages::GoAwayMessage {
            new_session_uri: new_session_uri.to_string(),
            request_id: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn a_second_goaway_on_a_request_stream_closes_the_session() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-request-goaway-twice", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;
        let stream = outgoing_stream(&client, subscription.request_id());
        stream.sender.send(&request_go_away("")).await.unwrap();

        // Act
        stream.sender.send(&request_go_away("")).await.unwrap();
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed())
            .await
            .unwrap();

        // Assert: §10.4 PROTOCOL_VIOLATION
        assert_eq!(close.code, Some(0x3), "{close:?}");
    }

    #[tokio::test]
    async fn a_goaway_with_a_new_session_uri_on_a_request_stream_to_the_server_closes_the_session()
    {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-request-goaway-uri", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;
        let stream = outgoing_stream(&client, subscription.request_id());

        // Act
        stream
            .sender
            .send(&request_go_away("moqt://elsewhere"))
            .await
            .unwrap();
        let close = tokio::time::timeout(DISCONNECT_TIMEOUT, client.closed())
            .await
            .unwrap();

        // Assert: §10.4 PROTOCOL_VIOLATION
        assert_eq!(close.code, Some(0x3), "{close:?}");
    }

    /// §12.4 DEFAULT PUBLISHER PRIORITY.
    fn default_priority_property(priority: u64) -> KeyValuePair {
        KeyValuePair {
            key: 0x0E,
            value: VariantType::Even(priority),
        }
    }

    /// A SUBSCRIBE from `client` that `server` answers itself with a
    /// SUBSCRIBE_OK for `track_alias` carrying `track_properties`. Hold the
    /// returned handler: dropping it unanswered sends REQUEST_ERROR.
    async fn subscribe_answered_with_track_properties(
        client: &crate::Session,
        server: &crate::Session,
        track_alias: u64,
        track_properties: &[KeyValuePair],
    ) -> (
        Result<crate::Subscription, RequestFailure>,
        SubscribeHandler,
    ) {
        let (request, handler) =
            subscribe_requested(client, server, SubscribeOption::default()).await;
        let stream = ControlChannel::of(&server.publisher().session.connection())
            .requests()
            .incoming
            .get(&handler.request_id())
            .cloned()
            .unwrap();
        let mut payload = BytesMut::new();
        codec::put_vi64(&mut payload, track_alias);
        parameters::put_parameters(&mut payload, &[]).unwrap();
        codec::put_key_value_pairs(&mut payload, track_properties).unwrap();
        stream
            .sender
            .send(&messages::frame(messages::SUBSCRIBE_OK, &payload).unwrap())
            .await
            .unwrap();
        (request.await.unwrap(), handler)
    }

    async fn subscribed_with_track_properties(
        client: &crate::Session,
        server: &crate::Session,
        track_alias: u64,
        track_properties: &[KeyValuePair],
    ) -> (crate::Subscription, SubscribeHandler) {
        let (subscription, handler) =
            subscribe_answered_with_track_properties(client, server, track_alias, track_properties)
                .await;
        (subscription.unwrap(), handler)
    }

    /// A PUBLISH of track `ns/track` that `publisher` sends with
    /// `track_properties`; returns the stream its answer arrives on.
    async fn raw_publish(
        publisher: &crate::Session,
        track_alias: u64,
        track_properties: &[KeyValuePair],
    ) -> (StreamSender, MessageReceiver) {
        let connection = publisher.publisher().session.connection();
        let (send_stream, receive_stream) = connection.transport.open_bi().await.unwrap();
        let mut payload = BytesMut::new();
        codec::put_vi64(&mut payload, connection.control.next_request_id());
        codec::put_full_track_name(&mut payload, &["ns".to_string()], "track");
        codec::put_vi64(&mut payload, track_alias);
        parameters::put_parameters(&mut payload, &[]).unwrap();
        codec::put_key_value_pairs(&mut payload, track_properties).unwrap();
        let sender = StreamSender::new(send_stream);
        sender
            .send(&messages::frame(messages::PUBLISH, &payload).unwrap())
            .await
            .unwrap();
        (sender, MessageReceiver::new(receive_stream, MessageDecoder))
    }

    async fn send_raw_stream(session: &crate::Session, bytes: &[u8]) {
        let connection = session.publisher().session.connection();
        let sender = StreamSender::new(connection.transport.open_uni().await.unwrap());
        sender.send(&BytesMut::from(bytes)).await.unwrap();
        sender.close().await.unwrap();
    }

    async fn next_stream_object(
        receiver: &mut DataReceiver,
    ) -> (crate::object::SubgroupStart, Object) {
        let mut stream = next_subgroup(receiver).await;
        let Some(Subgroup::Header(start)) = stream.receive().await.unwrap() else {
            panic!("expected the subgroup header");
        };
        let Some(Subgroup::Object(object)) = stream.receive().await.unwrap() else {
            panic!("expected an object");
        };
        (start, object)
    }

    #[tokio::test]
    async fn subgroups_inherit_the_default_publisher_priority_of_the_subscribe_ok() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-default-priority-stream", "moqt").await;
        let (subscription, _handler) =
            subscribed_with_track_properties(&client, &server, 4, &[default_priority_property(9)])
                .await;

        // Act: a header without priority (type 0x30), then one with 200 (0x10)
        send_raw_stream(&server, &[0x30, 4, 0, 0, 1, b'a']).await;
        let mut receiver = data_receiver(&client, &subscription).await;
        let (inherited_start, inherited) = next_stream_object(&mut receiver).await;
        send_raw_stream(&server, &[0x10, 4, 1, 200, 0, 1, b'b']).await;
        let (explicit_start, explicit) = next_stream_object(&mut receiver).await;

        // Assert
        assert_eq!(inherited_start.publisher_priority, 9);
        assert_eq!(inherited.publisher_priority, 9);
        assert_eq!(explicit_start.publisher_priority, 200);
        assert_eq!(explicit.publisher_priority, 200);
    }

    #[tokio::test]
    async fn a_datagram_without_priority_takes_the_track_default_or_128() {
        for (properties, expected) in [(vec![default_priority_property(9)], 9), (vec![], 128)] {
            // Arrange
            let (client, server) =
                spawn_connected_draft18_sessions("draft18-default-priority-datagram", "moqt").await;
            let (subscription, _handler) =
                subscribed_with_track_properties(&client, &server, 4, &properties).await;

            // Act: type 0x0C omits the priority and the Object ID
            server
                .publisher()
                .session
                .connection()
                .transport
                .send_datagram(BytesMut::from(&[0x0C, 4, 0, b'd'][..]))
                .unwrap();
            let object = next_datagram(&mut data_receiver(&client, &subscription).await).await;

            // Assert
            assert_eq!(object.publisher_priority, expected, "{properties:?}");
        }
    }

    #[tokio::test]
    async fn a_datagram_without_priority_takes_the_default_its_publish_announces() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-default-priority-publish", "moqt").await;
        let _publish_stream = raw_publish(&client, 6, &[default_priority_property(9)]).await;
        let subscription = accept_publish(&server).await;

        // Act
        client
            .publisher()
            .session
            .connection()
            .transport
            .send_datagram(BytesMut::from(&[0x0C, 6, 0, b'd'][..]))
            .unwrap();
        let object = next_datagram(&mut data_receiver(&server, &subscription).await).await;

        // Assert
        assert_eq!(object.publisher_priority, 9);
    }

    #[tokio::test]
    async fn a_max_cache_duration_inside_immutable_properties_reaches_the_subscription() {
        let max_cache_duration = KeyValuePair {
            key: 0x04,
            value: VariantType::Even(50),
        };
        let mut contents = BytesMut::new();
        codec::put_key_value_pairs(&mut contents, std::slice::from_ref(&max_cache_duration))
            .unwrap();
        let immutable = KeyValuePair {
            key: 0x0B,
            value: VariantType::Odd(contents.freeze()),
        };
        for properties in [vec![immutable], vec![max_cache_duration]] {
            // Arrange
            let (client, server) =
                spawn_connected_draft18_sessions("draft18-immutable-max-cache-duration", "moqt")
                    .await;

            // Act
            let (subscription, _handler) =
                subscribed_with_track_properties(&client, &server, 4, &properties).await;

            // Assert
            assert_eq!(
                subscription.track_properties().max_cache_duration,
                Some(50),
                "{properties:?}"
            );
        }
    }

    /// Track Properties moqt has no field for: Immutable Properties holding
    /// an application property, and an application property (§2.5).
    fn properties_without_a_field() -> Vec<KeyValuePair> {
        vec![
            messages::immutable_application_property(),
            KeyValuePair {
                key: 0x38,
                value: VariantType::Even(1),
            },
        ]
    }

    #[tokio::test]
    async fn track_properties_without_a_field_reach_the_subscription_as_received() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-other-track-properties", "moqt").await;

        // Act
        let (subscription, _handler) =
            subscribed_with_track_properties(&client, &server, 4, &properties_without_a_field())
                .await;

        // Assert
        assert_eq!(
            subscription.track_properties().other,
            properties_without_a_field()
        );
    }

    #[tokio::test]
    async fn a_subscribe_ok_carries_the_track_properties_held_as_received() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-send-other-track-properties", "moqt").await;
        let (_sender, mut receiver, mut handler) = raw_subscribe(&client, &server).await;
        handler.track_properties.other = properties_without_a_field();

        // Act
        handler.ok(0, ContentExists::False).await.unwrap();

        // Assert: DEFAULT PUBLISHER GROUP ORDER comes from the model
        let response = receiver.receive().await.unwrap();
        let Some(messages::Message::SubscribeOk(subscribe_ok)) = response else {
            panic!("expected SUBSCRIBE_OK, got {response:?}");
        };
        let mut expected = properties_without_a_field();
        expected.insert(
            1,
            KeyValuePair {
                key: 0x22,
                value: VariantType::Even(1),
            },
        );
        assert_eq!(subscribe_ok.track_properties, expected);
    }

    #[tokio::test]
    async fn a_subscribe_ok_with_a_mandatory_track_property_fails_the_subscribe() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-mandatory-subscribe-ok", "moqt").await;

        // Act
        let (result, _handler) = subscribe_answered_with_track_properties(
            &client,
            &server,
            4,
            &mandatory_track_properties().other,
        )
        .await;

        // Assert
        assert!(
            matches!(
                &result,
                Err(RequestFailure::Rejected {
                    code: RequestErrorCode::UnsupportedExtension,
                    ..
                })
            ),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_publish_with_a_mandatory_track_property_is_refused() {
        // Arrange
        let (client, _server) =
            spawn_connected_draft18_sessions("draft18-mandatory-publish", "moqt").await;

        // Act
        let (_sender, mut receiver) =
            raw_publish(&client, 6, &mandatory_track_properties().other).await;
        let response = receiver.receive().await.unwrap();

        // Assert
        let Some(messages::Message::RequestError(error)) = response else {
            panic!("expected REQUEST_ERROR, got {response:?}");
        };
        assert_eq!(error.code, RequestErrorCode::UnsupportedExtension);
    }

    #[tokio::test]
    async fn a_publish_with_malformed_immutable_properties_is_refused_before_the_application() {
        // Arrange: Immutable Properties holding Type 0x04 without its value
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-malformed-publish", "moqt").await;
        let properties = [KeyValuePair {
            key: 0x0B,
            value: VariantType::Odd(Bytes::from_static(&[0x04])),
        }];

        // Act
        let (_sender, mut receiver) = raw_publish(&client, 6, &properties).await;
        let response = receiver.receive().await.unwrap();

        // Assert: §12.7, §2.4.2; the application, e.g. a relay, never sees
        // the PUBLISH, so it has no track to latch
        let Some(messages::Message::RequestError(error)) = response else {
            panic!("expected REQUEST_ERROR, got {response:?}");
        };
        assert_eq!(error.code, RequestErrorCode::MalformedTrack);
        let event = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            server.receive_event(),
        )
        .await;
        assert!(event.is_err(), "{event:?}");
    }

    #[tokio::test]
    async fn a_subscribe_refused_with_malformed_track_fails_as_malformed_track() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-subscribe-malformed-track", "moqt").await;
        let (request, handler) =
            subscribe_requested(&client, &server, SubscribeOption::default()).await;

        // Act: the publisher, e.g. a relay that latched the track, refuses it
        handler
            .error(RequestErrorCode::MalformedTrack, "malformed".to_string())
            .await
            .unwrap();

        // Assert: §2.4.2, §10.6 MALFORMED_TRACK reaches the subscriber as such
        let result = request.await.unwrap();
        assert!(
            matches!(
                &result,
                Err(RequestFailure::Rejected {
                    code: RequestErrorCode::MalformedTrack,
                    ..
                })
            ),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn a_subscribe_ok_with_malformed_immutable_properties_fails_the_subscribe() {
        let immutable = |contents: Vec<u8>| KeyValuePair {
            key: 0x0B,
            value: VariantType::Odd(Bytes::from(contents)),
        };
        // Type 0x04 without its value; Immutable Properties (0x0B) of one
        // empty Immutable Properties.
        for properties in [
            vec![immutable(vec![0x04])],
            vec![immutable(vec![0x0B, 0x00])],
        ] {
            // Arrange
            let (client, server) =
                spawn_connected_draft18_sessions("draft18-malformed-immutable", "moqt").await;

            // Act
            let (result, _handler) =
                subscribe_answered_with_track_properties(&client, &server, 4, &properties).await;

            // Assert: §12.7
            assert!(
                matches!(
                    &result,
                    Err(RequestFailure::Rejected {
                        code: RequestErrorCode::MalformedTrack,
                        ..
                    })
                ),
                "{properties:?}: {result:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_goaway_on_a_namespace_subscription_withdraws_its_namespaces_once() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespace-goaway", "moqt").await;
        let subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe_namespace("live".into(), NamespaceInterest::Namespaces)
                .await
        });
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
            filter_type: FilterType::LargestObject,
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

        // Assert: §10.2.9 an omitted SUBSCRIPTION_FILTER leaves the filter unchanged
        let SessionEvent::SubscribeUpdate(update) = event else {
            panic!("expected SUBSCRIBE_UPDATE, got {event:?}");
        };
        assert_eq!(update.subscription_request_id(), subscription.request_id());
        assert!(!update.forward());
        assert_eq!(update.subscriber_priority(), 7);
        assert_eq!(update.filter(), None);
    }

    #[tokio::test]
    async fn a_request_update_hands_over_the_filter_it_carries() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-filter", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;
        let stream = outgoing_stream(&client, subscription.request_id());
        let update = request_update(
            client
                .publisher()
                .session
                .connection()
                .control
                .next_request_id(),
            &[parameters::Parameter::SubscriptionFilter(
                FilterType::NextGroupStart,
            )],
        );

        // Act
        stream.sender.send(&update).await.unwrap();
        let event = next_event(&server).await;

        // Assert: a relative filter is left for the publisher to resolve
        let SessionEvent::SubscribeUpdate(update) = event else {
            panic!("expected SUBSCRIBE_UPDATE, got {event:?}");
        };
        assert_eq!(update.filter(), Some(FilterType::NextGroupStart));
    }

    /// A SUBSCRIBE `client` sends on a stream it reads itself; returns the
    /// stream and the unanswered handler `server` receives.
    async fn raw_subscribe(
        client: &crate::Session,
        server: &crate::Session,
    ) -> (StreamSender, MessageReceiver, SubscribeHandler) {
        let connection = client.publisher().session.connection();
        let (send_stream, receive_stream) = connection.transport.open_bi().await.unwrap();
        let sender = StreamSender::new(send_stream);
        let receiver = MessageReceiver::new(receive_stream, MessageDecoder);
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
        (sender, receiver, handler)
    }

    /// A SUBSCRIBE `client` sends on a stream it reads itself, answered by
    /// `server`; returns the stream after its SUBSCRIBE_OK.
    async fn raw_subscription(
        client: &crate::Session,
        server: &crate::Session,
    ) -> (StreamSender, MessageReceiver) {
        let (sender, mut receiver, handler) = raw_subscribe(client, server).await;
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
        assert_eq!(told, UpdateAnswer::Accepted { largest_told: true });
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

    fn largest(group_id: u64, object_id: u64) -> Location {
        Location {
            group_id,
            object_id,
        }
    }

    async fn request_ok_largest(receiver: &mut MessageReceiver) -> Option<Location> {
        let response = tokio::time::timeout(HANDSHAKE_TIMEOUT, receiver.receive())
            .await
            .expect("no answer to the REQUEST_UPDATE")
            .unwrap();
        let Some(messages::Message::RequestOk(ok)) = response else {
            panic!("expected REQUEST_OK, got {response:?}");
        };
        ok.parameters.iter().find_map(|parameter| match parameter {
            parameters::Parameter::LargestObject(largest) => Some(*largest),
            _ => None,
        })
    }

    #[tokio::test]
    async fn a_second_request_update_is_read_while_the_first_is_unanswered() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-read-on", "moqt").await;
        let (sender, _receiver) = raw_subscription(&client, &server).await;
        let _first = requested_update(&client, &server, &sender).await;

        // Act
        let second = requested_update(&client, &server, &sender).await;

        // Assert
        assert!(second.forward());
    }

    #[tokio::test]
    async fn request_update_answers_go_out_in_the_order_the_updates_arrived() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-answer-order", "moqt").await;
        let (sender, mut receiver) = raw_subscription(&client, &server).await;
        let first = requested_update(&client, &server, &sender).await;
        let second = requested_update(&client, &server, &sender).await;

        // Act
        second.ok(Some(largest(5, 0)));
        first.ok(Some(largest(4, 0)));

        // Assert: §10.9, one answer per update in the order they arrived
        assert_eq!(request_ok_largest(&mut receiver).await, Some(largest(4, 0)));
        assert_eq!(request_ok_largest(&mut receiver).await, Some(largest(5, 0)));
    }

    #[tokio::test]
    async fn an_unanswered_request_update_gets_request_error_timeout() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-timeout", "moqt").await;
        let (sender, mut receiver) = raw_subscription(&client, &server).await;
        let handler = requested_update(&client, &server, &sender).await;
        tokio::time::pause();

        // Act
        let response = receiver.receive().await.unwrap();
        tokio::time::resume();
        let late = handler.ok(Some(largest(4, 0)));

        // Assert: §10.6 TIMEOUT; §10.9.1 the update failed
        let Some(messages::Message::RequestError(error)) = response else {
            panic!("expected REQUEST_ERROR, got {response:?}");
        };
        assert_eq!(error.code, RequestErrorCode::Timeout);
        assert_eq!(late, UpdateAnswer::Failed);
    }

    #[tokio::test]
    async fn a_peer_reset_is_noticed_while_a_request_update_answer_is_pending() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-update-reset", "moqt").await;
        let (sender, _receiver) = raw_subscription(&client, &server).await;
        let _unanswered = requested_update(&client, &server, &sender).await;

        // Act
        sender.reset(0).await.unwrap();
        let event = next_event(&server).await;

        // Assert: §3.3.2 the reset withdraws the subscription
        assert!(
            matches!(event, SessionEvent::Unsubscribe(_)),
            "expected UNSUBSCRIBE, got {event:?}"
        );
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

    /// A SUBSCRIBE_NAMESPACE for "live" that `client` sends on a stream it
    /// reads itself, or a SUBSCRIBE_TRACKS with `tracks`; returns the stream
    /// after `server` accepted it.
    async fn raw_namespace_subscription(
        client: &crate::Session,
        server: &crate::Session,
        tracks: bool,
    ) -> (StreamSender, MessageReceiver) {
        let connection = client.publisher().session.connection();
        let (send_stream, receive_stream) = connection.transport.open_bi().await.unwrap();
        let sender = StreamSender::new(send_stream);
        let mut receiver = MessageReceiver::new(receive_stream, MessageDecoder);
        let request_id = connection.control.next_request_id();
        let prefix = vec!["live".to_string()];
        let request = if tracks {
            messages::encode_subscribe_tracks(&messages::SubscribeTracksMessage {
                request_id,
                track_namespace_prefix: prefix,
                authorization_tokens: vec![],
                forward: true,
            })
        } else {
            messages::encode_subscribe_namespace(&crate::modules::moqt::control_plane::control_messages::messages::subscribe_namespace::SubscribeNamespace {
                request_id,
                track_namespace_prefix: prefix,
                authorization_token: vec![],
            })
        };
        sender.send(&request.unwrap()).await.unwrap();
        let SessionEvent::SubscribeNameSpace(handler) = next_event(server).await else {
            panic!("expected SUBSCRIBE_NAMESPACE");
        };
        handler.ok().await.unwrap();
        let response = receiver.receive().await.unwrap();
        assert!(
            matches!(response, Some(messages::Message::RequestOk(_))),
            "{response:?}"
        );
        (sender, receiver)
    }

    /// Sends a REQUEST_UPDATE moving the subscription on `stream` to
    /// `prefix` and returns the handler `server` receives for it.
    async fn requested_namespace_update(
        client: &crate::Session,
        server: &crate::Session,
        stream: &StreamSender,
        prefix: &[&str],
    ) -> crate::namespace::SubscribeNamespaceUpdateHandler {
        let request_id = client
            .publisher()
            .session
            .connection()
            .control
            .next_request_id();
        let prefix = prefix.iter().map(|field| field.to_string()).collect();
        stream
            .send(&request_update(
                request_id,
                &[parameters::Parameter::TrackNamespacePrefix(prefix)],
            ))
            .await
            .unwrap();
        let event = next_event(server).await;
        let SessionEvent::SubscribeNamespaceUpdate(handler) = event else {
            panic!("expected SubscribeNamespaceUpdate, got {event:?}");
        };
        handler
    }

    #[tokio::test]
    async fn an_accepted_namespace_prefix_update_announces_namespaces_under_the_new_prefix() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespace-update", "moqt").await;
        let (sender, mut receiver) = raw_namespace_subscription(&client, &server, false).await;
        let handler =
            requested_namespace_update(&client, &server, &sender, &["live", "room"]).await;

        // Act
        handler.ok().await;
        let answer = receiver.receive().await.unwrap();
        server
            .publisher()
            .publish_namespace("live/room/alice".into())
            .await
            .unwrap();
        let announced = receiver.receive().await.unwrap();

        // Assert: §10.9.2 suffixes follow the updated prefix after the REQUEST_OK
        assert_eq!(handler.track_namespace_prefix(), "live");
        assert_eq!(handler.new_track_namespace_prefix(), "live/room");
        assert_eq!(handler.new_track_namespace_prefix_tuple(), ["live", "room"]);
        assert_eq!(handler.interest(), NamespaceInterest::Namespaces);
        assert!(
            matches!(answer, Some(messages::Message::RequestOk(_))),
            "{answer:?}"
        );
        assert_eq!(
            announced,
            Some(messages::Message::Namespace(vec!["alice".to_string()]))
        );
    }

    #[tokio::test]
    async fn a_refused_namespace_prefix_update_ends_the_subscription() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-namespace-update-refused", "moqt").await;
        let (sender, mut receiver) = raw_namespace_subscription(&client, &server, true).await;
        let handler = requested_namespace_update(&client, &server, &sender, &["other"]).await;

        // Act
        handler
            .error(RequestErrorCode::NamespacePrefixOverlap, String::new())
            .await;
        let answer = receiver.receive().await.unwrap();
        let end = receiver.receive().await.unwrap();
        let event = next_event(&server).await;

        // Assert: §10.2.14 PREFIX_OVERLAP, and §10.9.1 closes the stream
        assert_eq!(handler.interest(), NamespaceInterest::Tracks);
        let Some(messages::Message::RequestError(error)) = answer else {
            panic!("expected REQUEST_ERROR, got {answer:?}");
        };
        assert_eq!(error.code, RequestErrorCode::NamespacePrefixOverlap);
        assert!(end.is_none(), "{end:?}");
        let SessionEvent::UnsubscribeNamespace(unsubscribed) = event else {
            panic!("expected UnsubscribeNamespace, got {event:?}");
        };
        assert_eq!(unsubscribed.track_namespace_prefix(), "live");
        assert_eq!(unsubscribed.interest, NamespaceInterest::Tracks);
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

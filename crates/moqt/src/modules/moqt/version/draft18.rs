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
    time::Duration,
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
            domains::session_context::SessionContext,
            version::{self, Notification, OutgoingRequest, OutgoingResponse},
        },
        transport::transport_send_stream::TransportSendError,
    },
};

use message_stream::{MessageDecoder, MessageReceiver, MessageSender};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a request stream was opened for, so a notification or the end of
/// the stream finds its meaning.
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
    /// The prefix of a namespace subscription, when `namespace` falls under it.
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
    sender: Arc<MessageSender>,
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
    /// Request IDs this side made up for events that need no response
    /// (a NAMESPACE raised as PUBLISH_NAMESPACE).
    synthetic_request_ids: HashSet<RequestId>,
}

/// Synthetic Request IDs count down from here, far above any the peer
/// would use, so they cannot collide with real ones.
const FIRST_SYNTHETIC_REQUEST_ID: RequestId = u64::MAX;

pub(crate) struct ControlChannel {
    control: MessageSender,
    request_id: AtomicU64,
    synthetic_request_id: AtomicU64,
    /// The parity of the peer's Request IDs (§10.1).
    peer_parity: u64,
    requests: std::sync::Mutex<Requests>,
}

fn cancelled() -> u64 {
    error_codes::stream_reset_code_to_wire(DataStreamResetCode::Cancelled)
}

impl ControlChannel {
    /// A client numbers its requests from 0 and a server from 1, each in
    /// steps of two.
    fn new(control: MessageSender, first_request_id: RequestId) -> Self {
        Self {
            control,
            request_id: AtomicU64::new(first_request_id),
            synthetic_request_id: AtomicU64::new(FIRST_SYNTHETIC_REQUEST_ID),
            peer_parity: 1 - first_request_id,
            requests: std::sync::Mutex::new(Requests::default()),
        }
    }

    fn of(session: &SessionContext) -> &Self {
        let version::ControlChannel::Draft18(channel) = &session.control else {
            unreachable!("draft-18 tasks run only on draft-18 sessions")
        };
        channel
    }

    fn requests(&self) -> std::sync::MutexGuard<'_, Requests> {
        self.requests
            .lock()
            .expect("draft-18 request registry poisoned")
    }

    pub(crate) fn next_request_id(&self) -> RequestId {
        self.request_id.fetch_add(2, Ordering::SeqCst)
    }

    /// The smallest Request ID the peer has not used yet, for GOAWAY.
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

    fn register_incoming(
        &self,
        request_id: RequestId,
        kind: RequestStreamKind,
        sender: Arc<MessageSender>,
    ) {
        self.requests()
            .incoming
            .insert(request_id, RequestStream { kind, sender });
    }

    fn synthetic_request_id(&self) -> RequestId {
        let request_id = self.synthetic_request_id.fetch_sub(1, Ordering::SeqCst);
        self.requests().synthetic_request_ids.insert(request_id);
        request_id
    }

    /// Drops what a peer's namespace subscription was sent once it ends.
    fn forget_advertised(&self, subscription_request_id: RequestId) {
        self.requests()
            .advertised
            .retain(|_, request_id| *request_id != subscription_request_id);
    }

    /// The peer's namespace subscription `namespace` falls under.
    fn namespace_subscription(&self, namespace: &[String]) -> Option<(RequestId, RequestStream)> {
        self.requests()
            .incoming
            .iter()
            .find(|(_, stream)| stream.kind.namespace_prefix_of(namespace).is_some())
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
        session: &Arc<SessionContext>,
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
        let (send_stream, receive_stream) = session
            .transport_connection
            .open_bi()
            .await
            .map_err(|_| RequestFailure::SessionClosed)?;
        let sender = Arc::new(MessageSender::new(send_stream));
        sender.send(&message).await?;
        self.requests().outgoing.insert(
            request_id,
            RequestStream {
                kind: kind.clone(),
                sender: sender.clone(),
            },
        );
        let (response_sender, receiver) = tokio::sync::oneshot::channel();
        request_streams::OutgoingRequestTask::run(
            Arc::downgrade(session),
            request_id,
            kind,
            MessageReceiver::new(receive_stream, MessageDecoder),
            response_sender,
            group_order.unwrap_or(GroupOrder::Ascending),
        );
        Ok(PendingResponse {
            receiver,
            cancel: CancelOnDrop {
                session: Arc::downgrade(session),
                request_id,
                armed: true,
            },
        })
    }

    async fn send_on_incoming(
        &self,
        request_id: RequestId,
        message: BytesMut,
        ends_request: bool,
    ) -> Result<(), TransportSendError> {
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
        let request_id = match &response {
            OutgoingResponse::SubscribeOk(ok) | OutgoingResponse::TrackStatusOk(ok) => {
                ok.request_id
            }
            OutgoingResponse::PublishOk(ok) => ok.request_id,
            OutgoingResponse::FetchOk(ok) => ok.request_id,
            OutgoingResponse::PublishNamespaceOk(ok)
            | OutgoingResponse::SubscribeNamespaceOk(ok) => ok.request_id,
            OutgoingResponse::Error { request_id, .. } => *request_id,
        };
        if self.requests().synthetic_request_ids.remove(&request_id) {
            return Ok(());
        }
        let encoded = |result: anyhow::Result<BytesMut>| {
            result.map_err(|error| TransportSendError::SessionError {
                reason: error.to_string(),
            })
        };
        match response {
            OutgoingResponse::SubscribeOk(ok) => {
                let message = encoded(messages::encode_subscribe_ok(&ok))?;
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::TrackStatusOk(ok) => {
                let message = encoded(messages::encode_track_status_ok(&ok))?;
                // §10.14: the stream ends with the response.
                self.send_on_incoming(ok.request_id, message, true).await
            }
            OutgoingResponse::PublishOk(ok) => {
                let message = encoded(messages::encode_publish_ok(&ok))?;
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::FetchOk(ok) => {
                let message = encoded(messages::encode_fetch_ok(&ok))?;
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::PublishNamespaceOk(ok)
            | OutgoingResponse::SubscribeNamespaceOk(ok) => {
                let message = encoded(messages::encode_empty_request_ok())?;
                self.send_on_incoming(ok.request_id, message, false).await
            }
            OutgoingResponse::Error {
                request_id,
                code,
                reason_phrase,
                ..
            } => {
                let message = encoded(messages::encode_request_error(code, &reason_phrase))?;
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
                let stream = self
                    .take_outgoing(unsubscribe.request_id)
                    .or_else(|| self.take_incoming(unsubscribe.request_id));
                if let Some(stream) = stream {
                    stream.sender.reset(cancelled()).await?;
                }
                Ok(())
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
                let message = messages::encode_request_update(update.request_id, &update).map_err(
                    |error| TransportSendError::SessionError {
                        reason: error.to_string(),
                    },
                )?;
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
                let message = messages::encode_publish_done(code, stream_count, &reason).map_err(
                    |error| TransportSendError::SessionError {
                        reason: error.to_string(),
                    },
                )?;
                stream.sender.send(&message).await?;
                stream.sender.close().await
            }
            Notification::FetchCancel(fetch_cancel) => {
                if let Some(stream) = self.take_outgoing(fetch_cancel.request_id) {
                    stream.sender.reset(cancelled()).await?;
                }
                Ok(())
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
                        .map_err(|error| {
                            TransportSendError::SessionError {
                                reason: error.to_string(),
                            }
                        })?;
                        stream.sender.send(&message).await?;
                    }
                    return Ok(());
                }
                let kind = RequestStreamKind::PublishNamespace(done.track_namespace);
                let stream = Self::take_by_kind(&mut self.requests().outgoing, &kind);
                if let Some(stream) = stream {
                    stream.sender.reset(cancelled()).await?;
                }
                Ok(())
            }
            Notification::PublishNamespaceCancel { namespace, .. } => {
                let kind = RequestStreamKind::PublishNamespace(namespace);
                let stream = Self::take_by_kind(&mut self.requests().incoming, &kind);
                if let Some(stream) = stream {
                    stream.sender.reset(cancelled()).await?;
                }
                Ok(())
            }
            Notification::UnsubscribeNamespace(unsubscribe) => {
                let kind =
                    RequestStreamKind::SubscribeNamespace(unsubscribe.track_namespace_prefix);
                let stream = Self::take_by_kind(&mut self.requests().outgoing, &kind);
                if let Some(stream) = stream {
                    stream.sender.reset(cancelled()).await?;
                }
                Ok(())
            }
            Notification::GoAway(go_away) => {
                let message = messages::encode_go_away(&messages::GoAwayMessage {
                    new_session_uri: go_away.new_session_uri,
                    timeout: 0,
                    request_id: Some(self.next_peer_request_id()),
                })
                .map_err(|error| TransportSendError::SessionError {
                    reason: error.to_string(),
                })?;
                self.control.send(&message).await
            }
            // draft-18 bounds requests with the QUIC stream limit instead.
            Notification::MaxRequestId(_) | Notification::RequestsBlocked(_) => Ok(()),
        }
    }
}

/// Resets the request stream when the caller stops waiting before a
/// success response: cancelling the stream is how draft-18 withdraws a
/// request (§3.3.2), so a late response needs no special handling.
struct CancelOnDrop {
    session: Weak<SessionContext>,
    request_id: RequestId,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let Some(stream) = ControlChannel::of(&session).take_outgoing(self.request_id) else {
            return;
        };
        executor::spawn("cancel draft-18 request", async move {
            if let Err(error) = stream.sender.reset(cancelled()).await {
                tracing::debug!(%error, "could not cancel an abandoned request");
            }
        });
    }
}

pub(crate) struct PendingResponse {
    receiver: tokio::sync::oneshot::Receiver<ResponseMessage>,
    cancel: CancelOnDrop,
}

impl PendingResponse {
    /// A response known without asking the peer.
    fn resolved(response: ResponseMessage) -> Self {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let _ = sender.send(response);
        Self {
            receiver,
            cancel: CancelOnDrop {
                session: Weak::new(),
                request_id: 0,
                armed: false,
            },
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

impl PendingResponse {
    /// Bounds the wait like draft-14 does; a timeout cancels the request.
    pub(super) async fn response(mut self) -> Result<ResponseMessage, RequestFailure> {
        match executor::timeout(RESPONSE_TIMEOUT, &mut self.receiver).await {
            Ok(Ok(response)) => {
                self.cancel.armed = !is_success(&response);
                Ok(response)
            }
            Ok(Err(_)) => Err(RequestFailure::SessionClosed),
            Err(_) => Err(RequestFailure::Timeout),
        }
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use super::{
        ControlChannel, codec,
        message_stream::{MessageDecoder, MessageReceiver, MessageSender},
        messages, parameters,
    };

    use crate::{
        RequestErrorCode, RequestFailure, SessionEvent, TrackWriter,
        fetch::{Fetch, FetchOption},
        modules::{
            moqt::version::Version,
            test_support::{
                HANDSHAKE_TIMEOUT, datagram_object, published_track_between, read_fetch_objects,
                spawn_connected_draft18_sessions, subscribed_track_reader,
            },
        },
        object::{
            ContentExists, DataReceiver, ForwardingPreference, Location, Object, ObjectStatus,
        },
        publish::PublishDoneCode,
        subscribe::{FilterType, SubscribeOption},
    };

    const SCHEMES: [&str; 2] = ["moqt", "https"];

    async fn next_event(session: &crate::Session) -> SessionEvent {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, session.receive_event())
            .await
            .expect("no session event")
            .unwrap()
    }

    #[tokio::test]
    async fn sessions_offering_only_draft18_speak_draft18_over_quic_and_web_transport() {
        for scheme in SCHEMES {
            // Act
            let (client, server) =
                spawn_connected_draft18_sessions("draft18-negotiate", scheme).await;

            // Assert
            assert_eq!(
                client.publisher().session.control.version(),
                Version::Draft18,
                "{scheme}"
            );
            assert_eq!(
                server.publisher().session.control.version(),
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
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".into(), "track".into(), SubscribeOption::default())
                .await
        });
        let SessionEvent::Subscribe(handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE");
        };

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
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".into(), "track".into(), SubscribeOption::default())
                .await
        });
        let SessionEvent::Subscribe(handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE");
        };
        handler.ok(0, ContentExists::False).await.unwrap();
        let subscription = request.await.unwrap().unwrap();

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
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".into(), "track".into(), SubscribeOption::default())
                .await
        });
        let SessionEvent::Subscribe(handler) = next_event(&server).await else {
            panic!("expected SUBSCRIBE");
        };

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

    /// A subscription from client to server, answered with SUBSCRIBE_OK.
    async fn subscribed(
        client: &crate::Session,
        server: &crate::Session,
        option: SubscribeOption,
    ) -> crate::Subscription {
        let mut subscriber = client.subscriber();
        let request = tokio::spawn(async move {
            subscriber
                .subscribe("ns".into(), "track".into(), option)
                .await
        });
        let SessionEvent::Subscribe(handler) = next_event(server).await else {
            panic!("expected SUBSCRIBE");
        };
        handler.ok(0, ContentExists::False).await.unwrap();
        request.await.unwrap().unwrap()
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

    #[tokio::test]
    async fn subscribe_tracks_reaches_the_publisher_as_subscribe_namespace() {
        // Arrange
        let (client, server) = spawn_connected_draft18_sessions("draft18-tracks", "moqt").await;
        let session = client.publisher().session;
        let (send_stream, receive_stream) = session.transport_connection.open_bi().await.unwrap();
        let sender = MessageSender::new(send_stream);
        let mut receiver = MessageReceiver::new(receive_stream, MessageDecoder);
        let mut payload = BytesMut::new();
        codec::put_vi64(&mut payload, session.control.next_request_id());
        codec::put_track_namespace(&mut payload, &["live".to_string()]);
        codec::put_vi64(&mut payload, 0);

        // Act
        sender
            .send(&messages::frame(messages::SUBSCRIBE_TRACKS, &payload).unwrap())
            .await
            .unwrap();
        let event = next_event(&server).await;
        let SessionEvent::SubscribeNameSpace(handler) = event else {
            panic!("expected SUBSCRIBE_NAMESPACE, got {event:?}");
        };
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
    async fn a_goaway_on_a_subscription_ends_it_with_going_away() {
        // Arrange
        let (client, server) =
            spawn_connected_draft18_sessions("draft18-request-goaway", "moqt").await;
        let subscription = subscribed(&client, &server, SubscribeOption::default()).await;
        let server_session = server.publisher().session;
        let stream = ControlChannel::of(&server_session)
            .requests()
            .incoming
            .get(&subscription.request_id())
            .cloned()
            .unwrap();
        let go_away = messages::encode_go_away(&messages::GoAwayMessage {
            new_session_uri: String::new(),
            timeout: 0,
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
        let client_session = client.publisher().session;
        let stream = ControlChannel::of(&client_session)
            .requests()
            .outgoing
            .get(&subscription.request_id())
            .cloned()
            .unwrap();
        let mut payload = BytesMut::new();
        codec::put_vi64(&mut payload, client_session.control.next_request_id());
        parameters::put_parameters(&mut payload, &[parameters::Parameter::Forward(false)]).unwrap();

        // Act: an update that changes only FORWARD
        stream
            .sender
            .send(&messages::frame(messages::REQUEST_UPDATE, &payload).unwrap())
            .await
            .unwrap();
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

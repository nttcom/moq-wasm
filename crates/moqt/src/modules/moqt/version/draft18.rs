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
}

pub(crate) struct ControlChannel {
    control: MessageSender,
    request_id: AtomicU64,
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
    use bytes::Bytes;

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
        subscribe::SubscribeOption,
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

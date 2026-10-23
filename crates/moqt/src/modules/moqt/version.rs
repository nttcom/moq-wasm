//! The per-draft adapters of a session. Everything above this module (the
//! `Session`, `Publisher`, `Subscriber`, the handlers) states what to send
//! and receive; the adapter decides how a draft carries it: which streams
//! hold control messages, how a response finds its request, how a request
//! is cancelled.
//!
//! draft-14 and draft-18 are implemented. `DEFAULT_VERSIONS` offers only
//! draft-14 because draft-18 has not been checked against other
//! implementations.

pub(crate) mod draft14;
pub(crate) mod draft18;

use std::{
    sync::{Arc, Weak},
    time::Duration,
};

use bytes::BytesMut;

use tokio::sync::mpsc::UnboundedSender;
use tracing::Span;

use crate::{
    RequestFailure,
    modules::{
        executor::JoinHandle,
        moqt::{
            control_plane::control_messages::messages::parameters::group_order::GroupOrder,
            control_plane::{
                constants::TerminationErrorCode,
                control_messages::messages::{
                    fetch::Fetch, fetch_cancel::FetchCancel, fetch_ok::FetchOk, go_away::GoAway,
                    max_request_id::MaxRequestId, namespace_ok::NamespaceOk,
                    parameters::authorization_token::AuthorizationToken, publish::Publish,
                    publish_namespace::PublishNamespace,
                    publish_namespace_done::PublishNamespaceDone, publish_ok::PublishOk,
                    requests_blocked::RequestsBlocked, server_setup::ServerSetup,
                    subscribe::Subscribe, subscribe_namespace::SubscribeNamespace,
                    subscribe_ok::SubscribeOk, subscribe_update::SubscribeUpdate,
                    unsubscribe::Unsubscribe,
                },
                enums::{RequestId, ResponseMessage},
                error_codes::{
                    DataStreamResetCode, PublishDoneCode, RequestErrorCode, RequestKind,
                },
                namespace_interest::NamespaceInterest,
            },
            data_plane::{
                object::canonical_object::{Object, SubgroupStart},
                stream::{
                    fetch_data_receiver::{self, FetchDataReceiver},
                    stream_data_receiver::StreamDataReceiver,
                    stream_receiver::{BiStreamReceiver, StreamReceiveError},
                    stream_sender::StreamSender,
                },
            },
            domains::{connection::Connection, session_context::SessionContext},
            runtime::dispatch::incoming_object::IncomingDatagram,
            version::draft14::control_message_receive_task::ControlMessageReceiveTask,
        },
        transport::{
            application_protocol::{ApplicationProtocols, NegotiatedProtocol},
            connect_target::ConnectTarget,
            transport_connection::{BoxedConnection, BoxedReceiveStream},
            transport_send_stream::TransportSendError,
        },
    },
};

/// draft-14 §3.1.2; SETUP then negotiates the version (§3.2).
const DRAFT14_ALPN: &str = "moq-00";

/// draft-18 §3.1.3 / §3.1.4: the WebTransport subprotocol and the ALPN.
const DRAFT18_PROTOCOL: &str = "moqt-18";

const CONTROL_MESSAGE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);

/// The drafts `ClientConfig::default()` offers.
pub const DEFAULT_VERSIONS: &[Version] = &[Version::Draft14];

/// A MoQT draft a session can speak; the one both ends offer is chosen when
/// the session is established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Version {
    /// draft-ietf-moq-transport-14.
    Draft14,
    /// draft-ietf-moq-transport-18.
    Draft18,
}

/// The ALPNs and WebTransport subprotocols that offer `versions`, in their order.
pub(crate) fn application_protocols(versions: &[Version]) -> anyhow::Result<ApplicationProtocols> {
    anyhow::ensure!(!versions.is_empty(), "no MoQT version to offer");
    Ok(ApplicationProtocols {
        quic_alpns: versions.iter().map(|version| version.alpn()).collect(),
        web_transport: versions
            .iter()
            .filter_map(|version| version.web_transport_protocol())
            .collect(),
    })
}

/// What a server accepting `versions` selects from. A WebTransport session
/// without a subprotocol is draft-14, so the subprotocols listed after
/// draft-14 are left out: the server's order decides there too.
pub(crate) fn server_application_protocols(
    versions: &[Version],
) -> anyhow::Result<ApplicationProtocols> {
    let mut protocols = application_protocols(versions)?;
    protocols.web_transport = versions
        .iter()
        .take_while(|version| **version != Version::Draft14)
        .filter_map(|version| version.web_transport_protocol())
        .collect();
    Ok(protocols)
}

impl Version {
    fn alpn(self) -> &'static str {
        match self {
            Self::Draft14 => DRAFT14_ALPN,
            Self::Draft18 => DRAFT18_PROTOCOL,
        }
    }

    fn web_transport_protocol(self) -> Option<&'static str> {
        match self {
            // draft-14 defines no WebTransport subprotocol (§3.1.1).
            Self::Draft14 => None,
            Self::Draft18 => Some(DRAFT18_PROTOCOL),
        }
    }

    pub(crate) fn termination_code(self, code: TerminationErrorCode) -> u32 {
        match self {
            Self::Draft14 => draft14::error_codes::termination_code_to_wire(code),
            Self::Draft18 => draft18::error_codes::termination_code_to_wire(code),
        }
    }

    /// Whether a track may carry objects on subgroup streams and in
    /// datagrams alike: draft-18 §11 allows it, draft-14 §10 forbids it.
    pub(crate) fn mixes_forwarding_preferences(self) -> bool {
        match self {
            Self::Draft14 => false,
            Self::Draft18 => true,
        }
    }

    /// draft-18 marks a datagram object in FETCH (§11.4.4); draft-14 does not (§10.4.4).
    pub(crate) fn fetch_carries_forwarding_preference(self) -> bool {
        match self {
            Self::Draft14 => false,
            Self::Draft18 => true,
        }
    }

    pub(crate) fn data_stream_reset_code(self, code: DataStreamResetCode) -> u64 {
        match self {
            Self::Draft14 => draft14::error_codes::data_stream_reset_code_to_wire(code),
            Self::Draft18 => draft18::error_codes::stream_reset_code_to_wire(code),
        }
    }

    fn negotiated(protocol: &NegotiatedProtocol) -> anyhow::Result<Self> {
        match protocol {
            NegotiatedProtocol::Quic { alpn } if alpn == DRAFT14_ALPN => Ok(Self::Draft14),
            NegotiatedProtocol::WebTransport { protocol: None } => Ok(Self::Draft14),
            NegotiatedProtocol::Quic { alpn } if alpn == DRAFT18_PROTOCOL => Ok(Self::Draft18),
            NegotiatedProtocol::WebTransport {
                protocol: Some(protocol),
            } if protocol == DRAFT18_PROTOCOL => Ok(Self::Draft18),
            other => anyhow::bail!("no supported MoQT version negotiated: {other:?}"),
        }
    }

    pub(crate) fn encode_fetch_header(self, request_id: RequestId) -> BytesMut {
        match self {
            Self::Draft14 => draft14::objects::encode_fetch_header(request_id),
            Self::Draft18 => draft18::objects::encode_fetch_header(request_id),
        }
    }

    pub(crate) fn encode_datagram(
        self,
        track_alias: u64,
        object: &Object,
    ) -> anyhow::Result<BytesMut> {
        match self {
            Self::Draft14 => draft14::objects::encode_datagram(track_alias, object),
            Self::Draft18 => draft18::objects::encode_datagram(track_alias, object),
        }
    }

    /// Returns `(track alias, datagram)`, or `None` for a datagram that
    /// carries no object (draft-18 padding).
    pub(crate) fn decode_datagram(
        self,
        data: &mut BytesMut,
    ) -> anyhow::Result<Option<(u64, IncomingDatagram)>> {
        match self {
            Self::Draft14 => draft14::objects::decode_datagram(data)
                .map(|(track_alias, object)| Some((track_alias, IncomingDatagram::new(object)))),
            Self::Draft18 => Ok(draft18::objects::decode_datagram(data)?),
        }
    }

    /// Reads the header of a unidirectional stream the peer opened.
    /// `Ok(None)` means the stream finished before its header.
    pub(crate) async fn accept_uni_stream(
        self,
        stream: BoxedReceiveStream,
    ) -> Result<Option<IncomingStream>, StreamReceiveError> {
        match self {
            Self::Draft14 => draft14::objects::accept_uni_stream(stream).await,
            Self::Draft18 => draft18::objects::accept_uni_stream(stream).await,
        }
    }
}

/// A unidirectional stream the peer opened, told apart by its header.
pub(crate) enum IncomingStream {
    Subgroup(StreamDataReceiver),
    Fetch(FetchDataReceiver),
}

#[derive(Debug)]
pub(crate) enum SubgroupReader {
    Draft14(draft14::objects::SubgroupReader),
    Draft18(draft18::objects::SubgroupReader),
}

impl SubgroupReader {
    /// `None` until the first object arrives, for header types that take the
    /// subgroup id from it.
    pub(crate) fn start(&self) -> Option<SubgroupStart> {
        match self {
            Self::Draft14(reader) => reader.start(),
            Self::Draft18(reader) => reader.start(),
        }
    }

    pub(crate) fn group_id(&self) -> u64 {
        match self {
            Self::Draft14(reader) => reader.group_id(),
            Self::Draft18(reader) => reader.group_id(),
        }
    }

    /// draft-14 headers always carry a priority.
    pub(crate) fn inherit_priority(&mut self, default_publisher_priority: u8) {
        match self {
            Self::Draft14(_) => {}
            Self::Draft18(reader) => reader.inherit_priority(default_publisher_priority),
        }
    }

    /// `Ok(None)` means the peer finished the stream (FIN).
    pub(crate) async fn next_object(&mut self) -> Result<Option<Object>, StreamReceiveError> {
        match self {
            Self::Draft14(reader) => reader.next_object().await,
            Self::Draft18(reader) => reader.next_object().await,
        }
    }
}

#[derive(Debug)]
pub(crate) enum FetchReader {
    Draft14(draft14::objects::FetchReader),
    Draft18(draft18::objects::FetchReader),
}

impl FetchReader {
    pub(crate) fn version(&self) -> Version {
        match self {
            Self::Draft14(_) => Version::Draft14,
            Self::Draft18(_) => Version::Draft18,
        }
    }

    /// draft-18 encodes Group IDs as deltas in the FETCH's group order.
    pub(crate) fn set_group_order(&mut self, group_order: GroupOrder) {
        match self {
            Self::Draft14(_) => {}
            Self::Draft18(reader) => reader.set_group_order(group_order),
        }
    }

    pub(crate) async fn next(&mut self) -> Result<fetch_data_receiver::Fetch, StreamReceiveError> {
        match self {
            Self::Draft14(reader) => Ok(reader.next_object().await?.map_or(
                fetch_data_receiver::Fetch::End,
                fetch_data_receiver::Fetch::Object,
            )),
            Self::Draft18(reader) => reader.next().await,
        }
    }
}

pub(crate) enum SubgroupEncoder {
    Draft14(draft14::objects::SubgroupEncoder),
    Draft18(draft18::objects::SubgroupEncoder),
}

impl SubgroupEncoder {
    pub(crate) fn new(version: Version, track_alias: u64) -> Self {
        match version {
            Version::Draft14 => Self::Draft14(draft14::objects::SubgroupEncoder::new(track_alias)),
            Version::Draft18 => Self::Draft18(draft18::objects::SubgroupEncoder::new(track_alias)),
        }
    }

    pub(crate) fn version(&self) -> Version {
        match self {
            Self::Draft14(_) => Version::Draft14,
            Self::Draft18(_) => Version::Draft18,
        }
    }

    pub(crate) fn header(&mut self, start: SubgroupStart) -> BytesMut {
        match self {
            Self::Draft14(encoder) => encoder.header(start),
            Self::Draft18(encoder) => encoder.header(start),
        }
    }

    /// Fails when the object id is not greater than the previous object's.
    pub(crate) fn object(&mut self, object: &Object) -> anyhow::Result<BytesMut> {
        match self {
            Self::Draft14(encoder) => encoder.object(object),
            Self::Draft18(encoder) => encoder.object(object),
        }
    }
}

/// The Object ID Delta of a subgroup object after `prev_object_id`, or its id
/// when it is the first.
fn object_id_delta(prev_object_id: Option<u64>, object_id: u64) -> anyhow::Result<u64> {
    match prev_object_id {
        None => Some(object_id),
        Some(prev) => prev
            .checked_add(1)
            .and_then(|next| object_id.checked_sub(next)),
    }
    .ok_or_else(|| {
        anyhow::anyhow!(
            "object {object_id} does not follow object {prev_object_id:?} in its subgroup"
        )
    })
}

/// Encodes the objects of one FETCH response.
pub(crate) enum FetchEncoder {
    Draft14,
    Draft18(draft18::objects::FetchEncoder),
}

impl FetchEncoder {
    pub(crate) fn new(version: Version) -> Self {
        match version {
            Version::Draft14 => Self::Draft14,
            Version::Draft18 => Self::Draft18(draft18::objects::FetchEncoder::default()),
        }
    }

    /// `Ok(None)` for an object the draft cannot carry in a FETCH response.
    pub(crate) fn object(&mut self, object: &Object) -> anyhow::Result<Option<BytesMut>> {
        match self {
            Self::Draft14 => Ok(Some(draft14::objects::encode_fetch_object(object))),
            Self::Draft18(encoder) => encoder.object(object),
        }
    }
}

pub(crate) enum OutgoingRequest {
    Subscribe(Subscribe),
    TrackStatus(Subscribe),
    Fetch(Fetch),
    Publish(Publish),
    PublishNamespace(PublishNamespace),
    SubscribeNamespace(SubscribeNamespace),
    /// draft-18's SUBSCRIBE_TRACKS; draft-14 sends it as SUBSCRIBE_NAMESPACE,
    /// which also asks for the tracks.
    SubscribeTracks(SubscribeNamespace),
}

pub(crate) enum OutgoingResponse {
    SubscribeOk(SubscribeOk),
    TrackStatusOk(SubscribeOk),
    PublishOk(PublishOk),
    FetchOk(FetchOk),
    PublishNamespaceOk(NamespaceOk),
    SubscribeNamespaceOk(NamespaceOk),
    Error {
        kind: RequestKind,
        request_id: RequestId,
        code: RequestErrorCode,
        reason_phrase: String,
    },
}

/// A control message that neither waits on nor answers a request.
pub(crate) enum Notification {
    Unsubscribe(Unsubscribe),
    SubscribeUpdate(SubscribeUpdate),
    PublishDone {
        request_id: RequestId,
        code: PublishDoneCode,
        stream_count: u64,
        reason: String,
    },
    FetchCancel(FetchCancel),
    PublishNamespaceDone(PublishNamespaceDone),
    PublishNamespaceCancel {
        namespace: Vec<String>,
        code: RequestErrorCode,
        reason: String,
    },
    /// Withdraws the namespace subscriptions for the prefix that cover
    /// `interest`.
    UnsubscribeNamespace {
        track_namespace_prefix: Vec<String>,
        interest: NamespaceInterest,
    },
    GoAway(GoAway),
    MaxRequestId(MaxRequestId),
    RequestsBlocked(RequestsBlocked),
}

/// How a session's control messages reach the peer.
pub(crate) enum ControlChannel {
    Draft14(draft14::ControlChannel),
    Draft18(Box<draft18::ControlChannel>),
}

impl ControlChannel {
    pub(crate) fn version(&self) -> Version {
        match self {
            Self::Draft14(_) => Version::Draft14,
            Self::Draft18(_) => Version::Draft18,
        }
    }

    pub(crate) fn next_request_id(&self) -> RequestId {
        match self {
            Self::Draft14(channel) => channel.next_request_id(),
            Self::Draft18(channel) => channel.next_request_id(),
        }
    }

    /// `None` for draft-14, whose FETCH has no request stream of its own.
    pub(crate) fn fetch_request_end(
        connection: &Arc<Connection>,
        request_id: RequestId,
    ) -> Option<draft18::FetchRequestEnd> {
        match connection.control {
            Self::Draft14(_) => None,
            Self::Draft18(_) => Some(draft18::FetchRequestEnd::new(
                Arc::downgrade(connection),
                request_id,
            )),
        }
    }

    /// Registers `response` as the response stream of the FETCH `request_id`
    /// the peer sent, so that draft-18 resets it when the FETCH is cancelled
    /// (§5.2). `false` when that FETCH is no longer open.
    pub(crate) fn attach_fetch_response(
        &self,
        request_id: RequestId,
        response: &Arc<StreamSender>,
    ) -> bool {
        match self {
            Self::Draft14(_) => true,
            Self::Draft18(channel) => channel.attach_fetch_response(request_id, response),
        }
    }

    /// The peer sent STOP_SENDING on the response stream of the FETCH
    /// `request_id` it sent: draft-18 cancels the FETCH (§5.2), while draft-14
    /// waits for its FETCH_CANCEL.
    pub(crate) async fn fetch_response_stopped(
        &self,
        connection: &Arc<Connection>,
        request_id: RequestId,
    ) {
        if let Self::Draft18(_) = self {
            draft18::ControlChannel::fetch_response_stopped(connection, request_id).await;
        }
    }

    /// The response stream of the FETCH `request_id` the peer sent ended
    /// with FIN or a reset (draft-18 §5.2).
    pub(crate) async fn fetch_response_ended(
        &self,
        request_id: RequestId,
    ) -> Result<(), TransportSendError> {
        match self {
            Self::Draft14(_) => Ok(()),
            Self::Draft18(channel) => channel.fetch_response_ended(request_id).await,
        }
    }

    /// draft-14 objects always carry a priority, so its value goes unused.
    pub(crate) fn default_publisher_priority(&self, request_id: RequestId) -> u8 {
        match self {
            Self::Draft14(_) => draft18::messages::DEFAULT_PUBLISHER_PRIORITY,
            Self::Draft18(channel) => channel.default_publisher_priority(request_id),
        }
    }

    /// The interests a namespace subscription of `interest` is sent as, one
    /// request each: draft-18 asks for namespaces with SUBSCRIBE_NAMESPACE and
    /// for tracks with SUBSCRIBE_TRACKS (draft-18 §6.1), while draft-14's
    /// single SUBSCRIBE_NAMESPACE covers both (draft-14 §6.1).
    pub(crate) fn namespace_requests(&self, interest: NamespaceInterest) -> Vec<NamespaceInterest> {
        match (self, interest) {
            (Self::Draft18(_), NamespaceInterest::NamespacesAndTracks) => {
                vec![NamespaceInterest::Namespaces, NamespaceInterest::Tracks]
            }
            _ => vec![interest],
        }
    }

    /// Fails with `RequestFailure::Send` when the request cannot be written.
    /// Dropping the returned `PendingResponse` abandons the request.
    /// `connection` owns this channel; draft-18 opens a stream on it per
    /// request.
    pub(crate) async fn send_request(
        &self,
        connection: &Arc<Connection>,
        request: OutgoingRequest,
    ) -> Result<PendingResponse, RequestFailure> {
        match self {
            Self::Draft14(channel) => Ok(PendingResponse::Draft14(
                channel.send_request(request).await?,
            )),
            Self::Draft18(channel) => Ok(PendingResponse::Draft18(
                channel.send_request(connection, request).await?,
            )),
        }
    }

    pub(crate) async fn respond(
        &self,
        response: OutgoingResponse,
    ) -> Result<(), TransportSendError> {
        match self {
            Self::Draft14(channel) => channel.respond(response).await,
            Self::Draft18(channel) => channel.respond(response).await,
        }
    }

    pub(crate) async fn notify(
        &self,
        notification: Notification,
    ) -> Result<(), TransportSendError> {
        match self {
            Self::Draft14(channel) => channel.notify(notification).await,
            Self::Draft18(channel) => channel.notify(notification).await,
        }
    }
}

/// The receiving half of a control channel, consumed when the session starts.
pub(crate) enum ControlReceiver {
    Draft14(BiStreamReceiver),
    Draft18(draft18::message_stream::MessageReceiver),
}

impl ControlReceiver {
    /// Spawns the task that reads the peer's control messages into session
    /// events and responses; the Request ID of every PUBLISH_DONE also goes to
    /// `publish_done_sender`.
    pub(crate) fn spawn(
        self,
        session: Weak<SessionContext>,
        connection: Weak<Connection>,
        publish_done_sender: UnboundedSender<RequestId>,
        span: Span,
    ) -> JoinHandle {
        match self {
            Self::Draft14(receive_stream) => ControlMessageReceiveTask::run(
                receive_stream,
                session,
                connection,
                publish_done_sender,
                span,
            ),
            Self::Draft18(control_stream) => draft18::request_streams::run_control_receiver(
                control_stream,
                session,
                connection,
                publish_done_sender,
                span,
            ),
        }
    }
}

fn negotiated_version(transport_connection: &BoxedConnection) -> anyhow::Result<Version> {
    let protocol = transport_connection.negotiated_protocol();
    let version = Version::negotiated(&protocol)?;
    tracing::debug!(?protocol, ?version, "MoQT version negotiated");
    Ok(version)
}

/// Opens a client session's control stream and runs the setup exchange.
pub(crate) async fn connect(
    transport_connection: &BoxedConnection,
    authorization_token: Option<&str>,
    target: &ConnectTarget,
) -> anyhow::Result<(ControlChannel, ControlReceiver, ServerSetup)> {
    match negotiated_version(transport_connection)? {
        Version::Draft14 => {
            let (channel, receiver, server_setup) =
                draft14::setup::connect(transport_connection, authorization_token).await?;
            Ok((
                ControlChannel::Draft14(channel),
                ControlReceiver::Draft14(receiver),
                server_setup,
            ))
        }
        Version::Draft18 => {
            let (channel, receiver, server_setup) =
                draft18::setup::connect(transport_connection, authorization_token, target).await?;
            Ok((
                ControlChannel::Draft18(Box::new(channel)),
                ControlReceiver::Draft18(receiver),
                server_setup,
            ))
        }
    }
}

/// Reads the client's setup so the server can decide before calling
/// `ServerHandshake::accept`.
pub(crate) async fn accept(
    transport_connection: &BoxedConnection,
) -> anyhow::Result<ServerHandshake> {
    match negotiated_version(transport_connection)? {
        Version::Draft14 => Ok(ServerHandshake::Draft14(
            draft14::setup::accept(transport_connection).await?,
        )),
        Version::Draft18 => Ok(ServerHandshake::Draft18(
            draft18::setup::accept(transport_connection).await?,
        )),
    }
}

pub(crate) enum ServerHandshake {
    Draft14(draft14::setup::ServerHandshake),
    Draft18(draft18::setup::ServerHandshake),
}

impl ServerHandshake {
    pub(crate) fn version(&self) -> Version {
        match self {
            Self::Draft14(_) => Version::Draft14,
            Self::Draft18(_) => Version::Draft18,
        }
    }

    pub(crate) fn authorization_tokens(&self) -> &[AuthorizationToken] {
        match self {
            Self::Draft14(handshake) => {
                &handshake.client_setup.setup_parameters.authorization_token
            }
            Self::Draft18(handshake) => &handshake.client_setup.authorization_tokens,
        }
    }

    /// Answers the client's setup and opens the session's control channel.
    pub(crate) async fn accept(
        self,
        transport_connection: &BoxedConnection,
    ) -> anyhow::Result<(ControlChannel, ControlReceiver)> {
        match self {
            Self::Draft14(handshake) => {
                let (channel, receiver) = handshake.accept().await?;
                Ok((
                    ControlChannel::Draft14(channel),
                    ControlReceiver::Draft14(receiver),
                ))
            }
            Self::Draft18(handshake) => {
                let (channel, receiver) = handshake.accept(transport_connection).await?;
                Ok((
                    ControlChannel::Draft18(Box::new(channel)),
                    ControlReceiver::Draft18(receiver),
                ))
            }
        }
    }
}

#[must_use = "dropping this abandons the request"]
pub(crate) enum PendingResponse {
    Draft14(draft14::PendingResponse),
    Draft18(draft18::PendingResponse),
}

impl PendingResponse {
    /// Fails with `RequestFailure::Timeout` after 10 s (the session stays
    /// open), or with `RequestFailure::SessionClosed` when the control stream
    /// closes first.
    pub(crate) async fn response(self) -> Result<ResponseMessage, RequestFailure> {
        match self {
            Self::Draft14(pending) => pending.response().await,
            Self::Draft18(pending) => pending.response().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Version, application_protocols};
    use crate::modules::transport::application_protocol::NegotiatedProtocol;

    #[test]
    fn both_drafts_are_offered_in_the_order_given() {
        // Act
        let protocols = application_protocols(&[Version::Draft18, Version::Draft14]).unwrap();

        // Assert
        assert_eq!(protocols.quic_alpns, ["moqt-18", "moq-00"]);
        assert_eq!(protocols.web_transport, ["moqt-18"]);
    }

    #[test]
    fn offering_no_version_fails() {
        // Act / Assert
        assert!(application_protocols(&[]).is_err());
    }

    #[test]
    fn the_draft14_alpn_selects_draft14() {
        // Act
        let version = Version::negotiated(&NegotiatedProtocol::Quic {
            alpn: "moq-00".to_string(),
        });

        // Assert
        assert_eq!(version.unwrap(), Version::Draft14);
    }

    #[test]
    fn a_web_transport_session_without_a_subprotocol_selects_draft14() {
        // Act
        let version = Version::negotiated(&NegotiatedProtocol::WebTransport { protocol: None });

        // Assert
        assert_eq!(version.unwrap(), Version::Draft14);
    }

    #[test]
    fn the_draft18_alpn_and_subprotocol_select_draft18() {
        // Act
        let over_quic = Version::negotiated(&NegotiatedProtocol::Quic {
            alpn: "moqt-18".to_string(),
        });
        let over_web_transport = Version::negotiated(&NegotiatedProtocol::WebTransport {
            protocol: Some("moqt-18".to_string()),
        });

        // Assert
        assert_eq!(over_quic.unwrap(), Version::Draft18);
        assert_eq!(over_web_transport.unwrap(), Version::Draft18);
    }

    #[test]
    fn an_unsupported_alpn_selects_no_version() {
        // Act
        let version = Version::negotiated(&NegotiatedProtocol::Quic {
            alpn: "moqt-99".to_string(),
        });

        // Assert
        assert!(version.is_err());
    }
}

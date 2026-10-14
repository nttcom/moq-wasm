//! The per-draft adapters of a session. Everything above this module (the
//! `Session`, `Publisher`, `Subscriber`, the handlers) states what to send
//! and receive; the adapter decides how a draft carries it: which streams
//! hold control messages, how a response finds its request, how a request
//! is cancelled.
//!
//! Only draft-14 is implemented. The adapter is in place ahead of draft-18
//! support, a deliberate exception to the coding-style rule that an
//! abstraction waits for its second concrete use.

pub(crate) mod draft14;

use std::sync::Weak;

use tracing::Span;

use crate::{
    RequestFailure,
    modules::{
        executor::JoinHandle,
        moqt::{
            control_plane::{
                control_messages::messages::{
                    fetch::Fetch, fetch_cancel::FetchCancel, fetch_ok::FetchOk, go_away::GoAway,
                    max_request_id::MaxRequestId, namespace_ok::NamespaceOk,
                    parameters::authorization_token::AuthorizationToken, publish::Publish,
                    publish_done::PublishDone, publish_namespace::PublishNamespace,
                    publish_namespace_cancel::PublishNamespaceCancel,
                    publish_namespace_done::PublishNamespaceDone, publish_ok::PublishOk,
                    request_error::RequestError, requests_blocked::RequestsBlocked,
                    server_setup::ServerSetup, subscribe::Subscribe,
                    subscribe_namespace::SubscribeNamespace, subscribe_ok::SubscribeOk,
                    subscribe_update::SubscribeUpdate, unsubscribe::Unsubscribe,
                    unsubscribe_namespace::UnsubscribeNamespace,
                },
                enums::{RequestId, ResponseMessage},
                error_codes::RequestKind,
            },
            data_plane::stream::stream_receiver::BiStreamReceiver,
            domains::session_context::SessionContext,
            version::draft14::control_message_receive_task::ControlMessageReceiveTask,
        },
        transport::{
            transport_connection::BoxedConnection, transport_send_stream::TransportSendError,
        },
    },
};

pub(crate) enum OutgoingRequest {
    Subscribe(Subscribe),
    TrackStatus(Subscribe),
    Fetch(Fetch),
    Publish(Publish),
    PublishNamespace(PublishNamespace),
    SubscribeNamespace(SubscribeNamespace),
}

pub(crate) enum OutgoingResponse {
    SubscribeOk(SubscribeOk),
    TrackStatusOk(SubscribeOk),
    PublishOk(PublishOk),
    FetchOk(FetchOk),
    PublishNamespaceOk(NamespaceOk),
    SubscribeNamespaceOk(NamespaceOk),
    Error(RequestKind, RequestError),
}

/// A control message that neither waits on nor answers a request.
pub(crate) enum Notification {
    Unsubscribe(Unsubscribe),
    SubscribeUpdate(SubscribeUpdate),
    PublishDone(PublishDone),
    FetchCancel(FetchCancel),
    PublishNamespaceDone(PublishNamespaceDone),
    PublishNamespaceCancel(PublishNamespaceCancel),
    UnsubscribeNamespace(UnsubscribeNamespace),
    GoAway(GoAway),
    MaxRequestId(MaxRequestId),
    RequestsBlocked(RequestsBlocked),
}

/// How a session's control messages reach the peer.
pub(crate) enum ControlChannel {
    Draft14(draft14::ControlChannel),
}

impl ControlChannel {
    pub(crate) fn next_request_id(&self) -> RequestId {
        match self {
            Self::Draft14(channel) => channel.next_request_id(),
        }
    }

    /// Fails with `RequestFailure::Send` when the request cannot be written.
    /// Dropping the returned `PendingResponse` abandons the request.
    pub(crate) async fn send_request(
        &self,
        request: OutgoingRequest,
    ) -> Result<PendingResponse, RequestFailure> {
        match self {
            Self::Draft14(channel) => Ok(PendingResponse::Draft14(
                channel.send_request(request).await?,
            )),
        }
    }

    pub(crate) async fn respond(
        &self,
        response: OutgoingResponse,
    ) -> Result<(), TransportSendError> {
        match self {
            Self::Draft14(channel) => channel.respond(response).await,
        }
    }

    pub(crate) async fn notify(
        &self,
        notification: Notification,
    ) -> Result<(), TransportSendError> {
        match self {
            Self::Draft14(channel) => channel.notify(notification).await,
        }
    }
}

/// The receiving half of a control channel, consumed when the session starts.
pub(crate) enum ControlReceiver {
    Draft14(BiStreamReceiver),
}

impl ControlReceiver {
    /// Spawns the task that reads the peer's control messages into session
    /// events and responses.
    pub(crate) fn spawn(self, session: Weak<SessionContext>, span: Span) -> JoinHandle {
        match self {
            Self::Draft14(receive_stream) => {
                ControlMessageReceiveTask::run(receive_stream, session, span)
            }
        }
    }
}

/// Opens a client session's control stream and runs the setup exchange.
pub(crate) async fn connect(
    transport_connection: &BoxedConnection,
    authorization_token: Option<&str>,
) -> anyhow::Result<(ControlChannel, ControlReceiver, ServerSetup)> {
    let (channel, receiver, server_setup) =
        draft14::setup::connect(transport_connection, authorization_token).await?;
    Ok((
        ControlChannel::Draft14(channel),
        ControlReceiver::Draft14(receiver),
        server_setup,
    ))
}

/// Reads the client's setup so the server can decide before calling
/// `ServerHandshake::accept`.
pub(crate) async fn accept(
    transport_connection: &BoxedConnection,
) -> anyhow::Result<ServerHandshake> {
    Ok(ServerHandshake::Draft14(
        draft14::setup::accept(transport_connection).await?,
    ))
}

pub(crate) enum ServerHandshake {
    Draft14(draft14::setup::ServerHandshake),
}

impl ServerHandshake {
    pub(crate) fn authorization_tokens(&self) -> &[AuthorizationToken] {
        match self {
            Self::Draft14(handshake) => {
                &handshake.client_setup.setup_parameters.authorization_token
            }
        }
    }

    /// Answers the client's setup and opens the session's control channel.
    pub(crate) async fn accept(self) -> anyhow::Result<(ControlChannel, ControlReceiver)> {
        match self {
            Self::Draft14(handshake) => {
                let (channel, receiver) = handshake.accept().await?;
                Ok((
                    ControlChannel::Draft14(channel),
                    ControlReceiver::Draft14(receiver),
                ))
            }
        }
    }
}

#[must_use = "dropping this abandons the request"]
pub(crate) enum PendingResponse {
    Draft14(draft14::PendingResponse),
}

impl PendingResponse {
    /// Fails with `RequestFailure::Timeout` after 10 s (the session stays
    /// open), or with `RequestFailure::SessionClosed` when the control stream
    /// closes first.
    pub(crate) async fn response(self) -> Result<ResponseMessage, RequestFailure> {
        match self {
            Self::Draft14(pending) => pending.response().await,
        }
    }
}

use crate::modules::moqt::version::OutgoingResponse;
use std::sync::Arc;

use crate::{
    modules::moqt::{
        control_plane::{
            control_messages::messages::{
                namespace_ok::NamespaceOk, subscribe_namespace::SubscribeNamespace,
            },
            error_codes::{RequestErrorCode, RequestKind},
            handler::response_guard::ResponseGuard,
            namespace_interest::NamespaceInterest,
        },
        domains::connection::Connection,
    },
    modules::transport::transport_send_stream::TransportSendError,
};

#[derive(Debug, Clone)]
pub struct SubscribeNamespaceHandler {
    pub track_namespace_prefix: String,
    pub track_namespace_prefix_tuple: Vec<String>,
    pub interest: NamespaceInterest,
    /// The Forward State of the PUBLISH requests this subscription leads to:
    /// the FORWARD parameter of a draft-18 SUBSCRIBE_TRACKS (§10.19), always
    /// `true` for draft-14.
    pub forward: bool,
    guard: ResponseGuard,
}

impl SubscribeNamespaceHandler {
    /// draft-14's SUBSCRIBE_NAMESPACE, which asks for namespaces and tracks.
    pub(crate) fn new(
        connection: Arc<Connection>,
        subscribe_namespace: SubscribeNamespace,
    ) -> Self {
        Self::with_interest(
            connection,
            subscribe_namespace,
            NamespaceInterest::NamespacesAndTracks,
            true,
        )
    }

    pub(crate) fn with_interest(
        connection: Arc<Connection>,
        subscribe_namespace: SubscribeNamespace,
        interest: NamespaceInterest,
        forward: bool,
    ) -> Self {
        let guard = ResponseGuard::new(
            connection,
            subscribe_namespace.request_id,
            RequestKind::SubscribeNamespace,
        );
        Self {
            guard,
            track_namespace_prefix: subscribe_namespace.track_namespace_prefix.join("/"),
            track_namespace_prefix_tuple: subscribe_namespace.track_namespace_prefix,
            interest,
            forward,
        }
    }

    pub async fn ok(&self) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let publish_namespace_ok = NamespaceOk {
            request_id: self.guard.request_id(),
        };
        self.guard
            .connection()
            .control
            .respond(OutgoingResponse::SubscribeNamespaceOk(publish_namespace_ok))
            .await?;
        Ok(())
    }

    pub async fn error(
        &self,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.reject(code, reason_phrase).await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SessionEvent,
        modules::test_support::{connect_sessions, spawn_dual_server},
        namespace::NamespaceInterest,
    };

    #[tokio::test]
    async fn exposes_track_namespace_prefix_as_tuple_and_joined_string() {
        // Arrange
        let (port, accept) = spawn_dual_server("subscribe-namespace-handler-namespace");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let request = tokio::spawn(async move {
            client
                .subscriber()
                .subscribe_namespace("a/b/c".to_string(), NamespaceInterest::NamespacesAndTracks)
                .await
        });

        // Act
        let SessionEvent::SubscribeNameSpace(handler) = server.receive_event().await.unwrap()
        else {
            panic!("expected SUBSCRIBE_NAMESPACE from the client");
        };

        // Assert
        assert_eq!(handler.track_namespace_prefix_tuple, vec!["a", "b", "c"]);
        assert_eq!(handler.track_namespace_prefix, "a/b/c");
        request.abort();
    }

    #[tokio::test]
    async fn a_draft14_subscribe_namespace_asks_for_namespaces_and_tracks_whatever_the_interest() {
        for interest in [NamespaceInterest::Namespaces, NamespaceInterest::Tracks] {
            // Arrange
            let (port, accept) = spawn_dual_server("subscribe-namespace-handler-interest");
            let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
                .await
                .unwrap();
            let request = tokio::spawn(async move {
                client
                    .subscriber()
                    .subscribe_namespace("room".to_string(), interest)
                    .await
            });

            // Act
            let SessionEvent::SubscribeNameSpace(handler) = server.receive_event().await.unwrap()
            else {
                panic!("expected SUBSCRIBE_NAMESPACE from the client");
            };

            // Assert: draft-14 §6.1 has one SUBSCRIBE_NAMESPACE, echoing PUBLISH back too
            assert_eq!(handler.interest, NamespaceInterest::NamespacesAndTracks);
            assert!(handler.forward);
            handler.ok().await.unwrap();
            assert!(request.await.unwrap().is_ok());
        }
    }
}

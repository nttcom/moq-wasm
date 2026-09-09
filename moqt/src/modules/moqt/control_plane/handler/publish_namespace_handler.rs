use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    namespace_ok::NamespaceOk, publish_namespace::PublishNamespace,
                    request_error::RequestError,
                },
            },
            handler::response_guard::ResponseGuard,
        },
        domains::session_context::SessionContext,
    },
    modules::transport::transport_send_stream::TransportSendError,
};

#[derive(Debug, Clone)]
pub struct PublishNamespaceHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    pub track_namespace: String,
    pub track_namespace_tuple: Vec<String>,
    guard: ResponseGuard<T>,
}

impl<T: TransportProtocol> PublishNamespaceHandler<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        publish_namespace: PublishNamespace,
    ) -> Self {
        let guard = ResponseGuard::new(
            session_context.clone(),
            publish_namespace.request_id,
            ControlMessageType::PublishNamespaceError,
        );
        Self {
            session_context,
            guard,
            request_id: publish_namespace.request_id,
            track_namespace: publish_namespace.track_namespace.join("/"),
            track_namespace_tuple: publish_namespace.track_namespace,
        }
    }

    pub async fn ok(&self) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let publish_namespace_ok = NamespaceOk {
            request_id: self.request_id,
        };
        self.session_context
            .send_stream
            .send(
                ControlMessageType::PublishNamespaceOk,
                publish_namespace_ok.encode(),
            )
            .await
    }

    pub async fn error(
        &self,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let err = RequestError {
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        self.session_context
            .send_stream
            .send(ControlMessageType::PublishNamespaceError, err.encode())
            .await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SessionEvent,
        modules::test_support::{connect_sessions, spawn_dual_server},
    };

    #[tokio::test]
    async fn exposes_track_namespace_as_tuple_and_joined_string() {
        // Arrange
        let (port, accept) = spawn_dual_server("publish-namespace-handler-namespace");
        let (client, server) = connect_sessions(&format!("moqt://127.0.0.1:{port}"), accept)
            .await
            .unwrap();
        let request = tokio::spawn(async move {
            client
                .publisher()
                .publish_namespace("a/b/c".to_string())
                .await
        });

        // Act
        let SessionEvent::PublishNamespace(handler) = server.receive_event().await.unwrap() else {
            panic!("expected PUBLISH_NAMESPACE from the client");
        };

        // Assert
        assert_eq!(handler.track_namespace_tuple, vec!["a", "b", "c"]);
        assert_eq!(handler.track_namespace, "a/b/c");
        request.abort();
    }
}

use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    namespace_ok::NamespaceOk, request_error::RequestError,
                    subscribe_namespace::SubscribeNamespace,
                },
            },
            handler::response_guard::ResponseGuard,
        },
        domains::session_context::SessionContext,
    },
    modules::transport::transport_send_stream::TransportSendError,
};

#[derive(Debug, Clone)]
pub struct SubscribeNamespaceHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    pub track_namespace_prefix: String,
    pub track_namespace_prefix_tuple: Vec<String>,
    guard: ResponseGuard<T>,
}

impl<T: TransportProtocol> SubscribeNamespaceHandler<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        subscribe_namespace: SubscribeNamespace,
    ) -> Self {
        let guard = ResponseGuard::new(
            session_context.clone(),
            subscribe_namespace.request_id,
            ControlMessageType::SubscribeNamespaceError,
        );
        Self {
            session_context,
            guard,
            request_id: subscribe_namespace.request_id,
            track_namespace_prefix: subscribe_namespace.track_namespace_prefix.join("/"),
            track_namespace_prefix_tuple: subscribe_namespace.track_namespace_prefix,
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
                ControlMessageType::SubscribeNamespaceOk,
                publish_namespace_ok.encode(),
            )
            .await?;
        Ok(())
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
            .send(ControlMessageType::SubscribeNamespaceError, err.encode())
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        SessionEvent,
        modules::test_support::{connect_sessions, spawn_dual_server},
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
                .subscribe_namespace("a/b/c".to_string())
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
}

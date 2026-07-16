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
    pub authorization_token: Option<String>,
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
            authorization_token: None,
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

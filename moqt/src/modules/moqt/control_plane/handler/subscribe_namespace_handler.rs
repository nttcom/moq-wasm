use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        control_plane::control_messages::{
            control_message_type::ControlMessageType,
            messages::{
                namespace_ok::NamespaceOk, request_error::RequestError,
                subscribe_namespace::SubscribeNamespace,
            },
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
    pub authorization_token: Option<String>,
}

impl<T: TransportProtocol> SubscribeNamespaceHandler<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        subscribe_namespace: SubscribeNamespace,
    ) -> Self {
        Self {
            session_context,
            request_id: subscribe_namespace.request_id,
            track_namespace_prefix: subscribe_namespace.track_namespace_prefix.join("/"),
            authorization_token: None,
        }
    }

    pub async fn ok(&self) -> Result<(), TransportSendError> {
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

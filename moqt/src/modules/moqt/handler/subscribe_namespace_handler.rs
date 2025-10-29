use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                namespace_ok::NamespaceOk, request_error::RequestError,
                subscribe_namespace::SubscribeNamespace,
            },
        },
        sessions::session_context::SessionContext,
        utils,
    },
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

    pub async fn ok(&self) -> anyhow::Result<()> {
        let publish_namespace_ok = NamespaceOk {
            request_id: self.request_id,
        };
        let bytes = utils::create_full_message(
            ControlMessageType::SubscribeNamespaceOk,
            publish_namespace_ok,
        );
        self.session_context.send_stream.send(&bytes).await
    }

    pub async fn error(&self, error_code: u64, reason_phrase: String) -> anyhow::Result<()> {
        let err = RequestError {
            // TODO: assign correct request id.
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        let bytes = utils::create_full_message(ControlMessageType::SubscribeNamespaceError, err);
        self.session_context.send_stream.send(&bytes).await
    }
}

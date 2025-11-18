use std::sync::Arc;

use crate::{
    TransportProtocol,
    modules::moqt::{
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                namespace_ok::NamespaceOk, publish_namespace::PublishNamespace,
                request_error::RequestError,
            },
        },
        sessions::session_context::SessionContext,
        utils,
    },
};

#[derive(Debug, Clone)]
pub struct PublishNamespaceHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    pub track_namespace: String,
    pub authorization_token: Option<String>,
}

impl<T: TransportProtocol> PublishNamespaceHandler<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        publish_namespace: PublishNamespace,
    ) -> Self {
        Self {
            session_context,
            request_id: publish_namespace.request_id,
            track_namespace: publish_namespace.track_namespace.join("/"),
            authorization_token: None,
        }
    }

    pub async fn ok(&self) -> anyhow::Result<()> {
        let publish_namespace_ok = NamespaceOk {
            request_id: self.request_id,
        };
        let bytes = utils::create_full_message(
            ControlMessageType::PublishNamespaceOk,
            publish_namespace_ok.encode(),
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
        let bytes =
            utils::create_full_message(ControlMessageType::PublishNamespaceError, err.encode());
        self.session_context.send_stream.send(&bytes).await
    }
}

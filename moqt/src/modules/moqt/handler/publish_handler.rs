use std::sync::Arc;

use crate::{
    GroupOrder, TransportProtocol,
    modules::moqt::{
        messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                location::Location, namespace_ok::NamespaceOk, publish::Publish,
                request_error::RequestError,
            },
        },
        sessions::session_context::SessionContext,
        utils,
    },
};

#[derive(Debug, Clone)]
pub struct PublishHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    pub track_namespace: String,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub content_exists: bool,
    pub largest_location: Option<Location>,
    pub forward: bool,
    pub authorization_token: Option<String>,
    pub max_cache_duration: Option<u64>,
    pub delivery_timeout: Option<u64>,
}

impl<T: TransportProtocol> PublishHandler<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, publish_message: Publish) -> Self {
        Self {
            session_context,
            request_id: publish_message.request_id,
            track_namespace: publish_message.track_namespace_tuple.join("/"),
            track_name: publish_message.track_name,
            track_alias: publish_message.track_alias,
            group_order: publish_message.group_order,
            content_exists: publish_message.content_exists,
            largest_location: publish_message.largest_location,
            forward: publish_message.forward,
            authorization_token: None,
            max_cache_duration: None,
            delivery_timeout: None,
        }
    }

    pub async fn ok(&self) -> anyhow::Result<()> {
        let publish_namespace_ok = NamespaceOk {
            request_id: self.request_id,
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishOk, publish_namespace_ok);
        self.session_context.send_stream.send(&bytes).await
    }

    pub async fn error(&self, error_code: u64, reason_phrase: String) -> anyhow::Result<()> {
        let err = RequestError {
            // TODO: assign correct request id.
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishError, err);
        self.session_context.send_stream.send(&bytes).await
    }
}

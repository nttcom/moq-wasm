use std::sync::Arc;

use crate::{
    FilterType, GroupOrder, Subscription, TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    parameters::content_exists::ContentExists, publish::Publish,
                    publish_ok::PublishOk, request_error::RequestError,
                },
            },
            utils,
        },
        domains::session_context::SessionContext,
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
    pub content_exists: ContentExists,
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
            forward: publish_message.forward,
            authorization_token: None,
            max_cache_duration: None,
            delivery_timeout: None,
        }
    }

    pub async fn ok(&self, subscriber_priority: u8, filter_type: FilterType) -> anyhow::Result<()> {
        let publish_ok = PublishOk {
            request_id: self.request_id,
            forward: self.forward,
            subscriber_priority,
            group_order: self.group_order,
            filter_type,
            delivery_timeout: self.delivery_timeout,
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishOk, publish_ok.encode());
        self.session_context.send_stream.send(&bytes).await
    }

    pub async fn error(&self, error_code: u64, reason_phrase: String) -> anyhow::Result<()> {
        let err = RequestError {
            // TODO: assign correct request id.
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        let bytes = utils::create_full_message(ControlMessageType::PublishError, err.encode());
        self.session_context.send_stream.send(&bytes).await
    }

    pub fn into_subscription(&self, expires: u64) -> Subscription {
        Subscription {
            track_alias: self.track_alias,
            expires,
            group_order: self.group_order,
            content_exists: self.content_exists,
            delivery_timeout: self.delivery_timeout,
        }
    }
}

use std::sync::Arc;

use crate::{
    FilterType, GroupOrder, PublishedResource, TransportProtocol,
    modules::moqt::{
        control_plane::messages::{
            control_message_type::ControlMessageType,
            control_messages::{
                enums::ContentExists, request_error::RequestError, subscribe::Subscribe,
                subscribe_ok::SubscribeOk,
            },
        },
        control_plane::models::session_context::SessionContext,
        control_plane::utils,
    },
};

#[derive(Debug, Clone)]
pub struct SubscribeHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    request_id: u64,
    pub track_namespace: String,
    pub track_name: String,
    pub subscriber_priority: u8,
    pub group_order: GroupOrder,
    pub forward: bool,
    pub filter_type: FilterType,
    pub authorization_token: Option<String>,
    pub max_cache_duration: Option<u64>,
    pub delivery_timeout: Option<u64>,
}

impl<T: TransportProtocol> SubscribeHandler<T> {
    pub(crate) fn new(
        session_context: Arc<SessionContext<T>>,
        subscribe_message: Subscribe,
    ) -> Self {
        Self {
            session_context,
            request_id: subscribe_message.request_id,
            track_namespace: subscribe_message.track_namespace.join("/"),
            track_name: subscribe_message.track_name,
            subscriber_priority: subscribe_message.subscriber_priority,
            group_order: subscribe_message.group_order,
            forward: subscribe_message.forward,
            filter_type: subscribe_message.filter_type,
            authorization_token: None,
            max_cache_duration: None,
            delivery_timeout: None,
        }
    }

    pub async fn ok(
        &self,
        track_alias: u64,
        expires: u64,
        content_exists: ContentExists,
    ) -> anyhow::Result<()> {
        let subscribe_ok = SubscribeOk {
            request_id: self.request_id,
            track_alias,
            expires,
            group_order: self.group_order,
            content_exists,
            subscribe_parameters: vec![],
        };
        let bytes =
            utils::create_full_message(ControlMessageType::SubscribeOk, subscribe_ok.encode());
        self.session_context.send_stream.send(&bytes).await
    }

    pub async fn error(&self, error_code: u64, reason_phrase: String) -> anyhow::Result<()> {
        let err = RequestError {
            // TODO: assign correct request id.
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        let bytes = utils::create_full_message(ControlMessageType::SubscribeError, err.encode());
        self.session_context.send_stream.send(&bytes).await
    }

    pub fn into_publication(&self, track_alias: u64) -> PublishedResource<T> {
        PublishedResource::from_subscribe_handler(self.session_context.clone(), track_alias, self)
    }
}

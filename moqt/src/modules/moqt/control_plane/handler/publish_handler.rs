use std::sync::Arc;

use crate::{
    FilterType, GroupOrder, PublisherInitiatedSubscription, Subscription, TransportProtocol,
    modules::moqt::{
        control_plane::{
            control_messages::{
                control_message_type::ControlMessageType,
                messages::{
                    parameters::content_exists::ContentExists, publish::Publish,
                    publish_ok::PublishOk, request_error::RequestError,
                },
            },
            handler::response_guard::ResponseGuard,
        },
        domains::session_context::SessionContext,
    },
    modules::transport::transport_send_stream::TransportSendError,
};

#[derive(Debug, Clone)]
pub struct PublishHandler<T: TransportProtocol> {
    session_context: Arc<SessionContext<T>>,
    pub request_id: u64,
    pub track_namespace: String,
    pub track_name: String,
    pub track_alias: u64,
    pub group_order: GroupOrder,
    pub content_exists: ContentExists,
    pub forward: bool,
    pub authorization_token: Option<String>,
    pub max_cache_duration: Option<u64>,
    pub delivery_timeout: Option<u64>,
    guard: ResponseGuard<T>,
}

impl<T: TransportProtocol> PublishHandler<T> {
    pub(crate) fn new(session_context: Arc<SessionContext<T>>, publish_message: Publish) -> Self {
        let guard = ResponseGuard::new(
            session_context.clone(),
            publish_message.request_id,
            ControlMessageType::PublishError,
        );
        Self {
            session_context,
            guard,
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

    pub async fn ok(
        &self,
        subscriber_priority: u8,
        filter_type: FilterType,
        expires: u64,
    ) -> Result<Subscription, TransportSendError> {
        self.guard.mark_responded();
        let publish_ok = PublishOk {
            request_id: self.request_id,
            forward: self.forward,
            subscriber_priority,
            group_order: self.group_order,
            filter_type,
            delivery_timeout: self.delivery_timeout,
        };
        self.session_context
            .send_stream
            .send(ControlMessageType::PublishOk, publish_ok.encode())
            .await?;

        let _ = expires;
        Ok(Subscription::PublisherInitiated(
            PublisherInitiatedSubscription {
                request_id: self.request_id,
                track_namespace: self.track_namespace.clone(),
                track_name: self.track_name.clone(),
                track_alias: self.track_alias,
                group_order: self.group_order,
                content_exists: self.content_exists,
                subscriber_priority,
                forward: self.forward,
                filter_type,
                delivery_timeout: self.delivery_timeout,
            },
        ))
    }

    pub async fn accept_data_receiver(&self) {
        if let Err(code) = self
            .session_context
            .register_data_receiver(self.track_alias)
            .await
        {
            // The track alias is already bound to another active subscription.
            // draft-14 §9.8: close the session with DUPLICATE_TRACK_ALIAS.
            tracing::error!(
                track_alias = self.track_alias,
                "PUBLISH reused an in-use track alias; closing session"
            );
            self.session_context
                .close_with_error(code, "PUBLISH reused an in-use track alias");
            return;
        }
        tracing::info!(
            track_alias = self.track_alias,
            "publish handler registered incoming object receiver"
        );
    }

    pub async fn error(
        &self,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
        self.guard.mark_responded();
        let err = RequestError {
            // TODO: assign correct request id.
            request_id: self.request_id,
            error_code,
            reason_phrase,
        };
        self.session_context
            .send_stream
            .send(ControlMessageType::PublishError, err.encode())
            .await?;
        Ok(())
    }
}

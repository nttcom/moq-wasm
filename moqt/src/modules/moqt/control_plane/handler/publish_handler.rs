use std::sync::Arc;

use crate::{
    FilterType, GroupOrder, PublisherInitiatedSubscription, Subscription, TransportProtocol,
    modules::moqt::{
        control_plane::control_messages::{
            control_message_type::ControlMessageType,
            messages::{
                parameters::content_exists::ContentExists, publish::Publish, publish_ok::PublishOk,
                request_error::RequestError,
            },
        },
        domains::session_context::SessionContext,
        runtime::dispatch::incoming_object::IncomingObject,
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

    pub async fn ok(
        &self,
        subscriber_priority: u8,
        filter_type: FilterType,
        expires: u64,
    ) -> Result<Subscription, TransportSendError> {
        self.prepare_data_receiver().await;

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

    pub async fn prepare_data_receiver(&self) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<IncomingObject<T>>();
        let registration = self
            .session_context
            .register_incoming_object_receiver(self.track_alias, sender)
            .await;
        if registration.already_registered {
            tracing::info!(
                track_alias = self.track_alias,
                "publish handler incoming object receiver is already registered"
            );
            return;
        }
        if registration.pending_objects > 0 {
            tracing::info!(
                track_alias = self.track_alias,
                pending_objects = registration.pending_objects,
                "draining pending incoming objects after PUBLISH receiver registration"
            );
        }
        if registration.failed_to_drain {
            tracing::warn!(
                track_alias = self.track_alias,
                "failed to drain pending incoming object"
            );
        }
        let replaced_receiver = self
            .session_context
            .receiver_map
            .lock()
            .await
            .insert(self.track_alias, receiver)
            .is_some();
        tracing::info!(
            track_alias = self.track_alias,
            replaced_notification_sender = false,
            replaced_receiver,
            "publish handler registered incoming object receiver"
        );
    }

    pub async fn error(
        &self,
        error_code: u64,
        reason_phrase: String,
    ) -> Result<(), TransportSendError> {
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

mod publish_namespace_response_task;

use publish_namespace_response_task::PublishNamespaceResponseTask;

use std::sync::Arc;

use moqt::{
    object::{ContentExists, GroupOrder},
    subscribe::{FilterType, SubscribeOption},
};

use crate::modules::{
    domain::{pub_sub_directory::entry::PublishDoneReason, session_id::SessionId},
    session::{
        publisher::Publisher,
        session_repository::SessionRepository,
        subscriber::Subscriber,
        subscription::{DownstreamSubscription, UpstreamSubscription},
    },
};

#[derive(Clone)]
pub(crate) struct ControlMessageForwarder {
    pub(crate) repository: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl ControlMessageForwarder {
    async fn publisher(&self, session_id: SessionId) -> anyhow::Result<Box<dyn Publisher>> {
        let publisher = self.repository.lock().await.publisher(session_id);
        publisher.ok_or_else(|| {
            tracing::debug!(session_id = %session_id, "No publisher");
            anyhow::anyhow!("No publisher")
        })
    }

    async fn subscriber(&self, session_id: SessionId) -> anyhow::Result<Box<dyn Subscriber>> {
        let subscriber = self.repository.lock().await.subscriber(session_id);
        subscriber.ok_or_else(|| {
            tracing::debug!(session_id = %session_id, "No subscriber");
            anyhow::anyhow!("No subscriber")
        })
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace)
    )]
    pub(crate) async fn publish_namespace(&self, session_id: SessionId, track_namespace: String) {
        let Ok(publisher) = self.publisher(session_id).await else {
            return;
        };
        match publisher.send_publish_namespace(track_namespace).await {
            Ok(response) => {
                let _response_task = PublishNamespaceResponseTask::run(response);
                tracing::debug!("forwarded PUBLISH_NAMESPACE");
            }
            Err(error) => tracing::error!(?error, "Failed to send publish namespace"),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish_namespace_done",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace)
    )]
    pub(crate) async fn publish_namespace_done(
        &self,
        session_id: SessionId,
        track_namespace: String,
    ) {
        let Ok(publisher) = self.publisher(session_id).await else {
            return;
        };
        match publisher.send_publish_namespace_done(track_namespace).await {
            Ok(()) => tracing::debug!("forwarded PUBLISH_NAMESPACE_DONE"),
            Err(error) => tracing::error!(?error, "Failed to send publish namespace done"),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn publish(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
        content_exists: ContentExists,
        max_cache_duration: Option<u64>,
    ) -> anyhow::Result<DownstreamSubscription> {
        let publisher = self.publisher(session_id).await?;
        publisher
            .send_publish(
                track_namespace,
                track_name,
                content_exists,
                max_cache_duration,
            )
            .await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish_done",
        skip_all,
        fields(session_id = %session_id, request_id = %subscription.request_id())
    )]
    pub(crate) async fn publish_done(
        &self,
        session_id: SessionId,
        subscription: &DownstreamSubscription,
        reason: PublishDoneReason,
    ) -> anyhow::Result<()> {
        let publisher = self.publisher(session_id).await?;
        publisher
            .send_publish_done(subscription, reason.status_code, 0, reason.error_reason)
            .await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.subscribe",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn subscribe(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<UpstreamSubscription> {
        let mut subscriber = self.subscriber(session_id).await?;
        let option = SubscribeOption {
            subscriber_priority: 128,
            group_order: GroupOrder::Ascending,
            forward: true,
            filter_type: FilterType::LargestObject,
        };
        tracing::debug!("forwarding SUBSCRIBE");
        subscriber
            .send_subscribe(track_namespace, track_name, option)
            .await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.fetch",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn fetch(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
        start_location: moqt::object::Location,
        end_location: moqt::object::Location,
        option: moqt::fetch::FetchOption,
    ) -> anyhow::Result<moqt::fetch::FetchHandle> {
        let mut subscriber = self.subscriber(session_id).await?;
        tracing::debug!("forwarding FETCH");
        let handle = subscriber
            .send_fetch(
                track_namespace,
                track_name,
                start_location,
                end_location,
                option,
            )
            .await?;
        Ok(handle)
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.unsubscribe",
        skip_all,
        fields(session_id = %session_id, request_id = %subscription.request_id())
    )]
    pub(crate) async fn unsubscribe(
        &self,
        session_id: SessionId,
        subscription: &UpstreamSubscription,
    ) -> anyhow::Result<()> {
        let subscriber = self.subscriber(session_id).await?;
        tracing::debug!("forwarding UNSUBSCRIBE");
        subscriber.send_unsubscribe(subscription).await
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.unsubscribe_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) async fn unsubscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: String,
    ) -> anyhow::Result<()> {
        let subscriber = self.subscriber(session_id).await?;
        tracing::debug!("forwarding UNSUBSCRIBE_NAMESPACE");
        subscriber
            .send_unsubscribe_namespace(track_namespace_prefix)
            .await
    }
}

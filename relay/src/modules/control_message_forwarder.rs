use std::sync::Arc;

use moqt::{FilterType, GroupOrder};

use crate::modules::{
    core::{
        handler::publish::SubscribeOption, publisher::Publisher, subscriber::Subscriber,
        subscription::UpstreamSubscription,
    },
    session_repository::SessionRepository,
    types::SessionId,
};

#[derive(Clone)]
pub(crate) struct ControlMessageForwarder {
    pub(crate) repository: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl ControlMessageForwarder {
    async fn publisher(&self, session_id: SessionId) -> Option<Box<dyn Publisher>> {
        let publisher = self.repository.lock().await.publisher(session_id);
        if publisher.is_none() {
            tracing::error!("No publisher");
        }
        publisher
    }

    async fn subscriber(&self, session_id: SessionId) -> anyhow::Result<Box<dyn Subscriber>> {
        let subscriber = self.repository.lock().await.subscriber(session_id);
        subscriber.ok_or_else(|| {
            tracing::error!("No subscriber");
            anyhow::anyhow!("No subscriber")
        })
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.publish_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace)
    )]
    pub(crate) async fn publish_namespace(
        &self,
        session_id: SessionId,
        track_namespace: String,
    ) -> bool {
        let Some(publisher) = self.publisher(session_id).await else {
            return false;
        };
        publisher
            .send_publish_namespace(track_namespace)
            .await
            .inspect_err(|_| tracing::error!("Failed to send publish namespace"))
            .is_ok()
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
    ) -> bool {
        let Some(publisher) = self.publisher(session_id).await else {
            return false;
        };
        publisher
            .send_publish_namespace_done(track_namespace)
            .await
            .inspect_err(|_| tracing::error!("Failed to send publish namespace done"))
            .is_ok()
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
    ) -> Option<u64> {
        let publisher = self.publisher(session_id).await?;
        match publisher
            .send_publish(track_namespace.clone(), track_name)
            .await
        {
            Ok(published_resource) => {
                tracing::info!(
                    "Forwarded PUBLISH '{}' to session:{}",
                    track_namespace,
                    session_id
                );
                Some(published_resource.track_alias())
            }
            Err(_) => {
                tracing::error!("Failed to send publish namespace");
                None
            }
        }
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
        tracing::info!(
            "Forwarded SUBSCRIBE '{}' to session:{}",
            track_namespace,
            session_id
        );
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
        start_location: moqt::Location,
        end_location: moqt::Location,
        option: moqt::FetchOption,
    ) -> anyhow::Result<moqt::FetchHandle> {
        let mut subscriber = self.subscriber(session_id).await?;
        tracing::info!(
            "Forwarding FETCH '{}/{}' to session:{}",
            track_namespace,
            track_name,
            session_id
        );
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
        fields(session_id = %session_id, subscribe_id = %subscribe_id)
    )]
    pub(crate) async fn unsubscribe(
        &self,
        session_id: SessionId,
        subscribe_id: u64,
    ) -> anyhow::Result<()> {
        let subscriber = self.subscriber(session_id).await?;
        tracing::info!(
            "Forwarded UNSUBSCRIBE subscribe_id={} to session:{}",
            subscribe_id,
            session_id
        );
        subscriber.send_unsubscribe(subscribe_id).await
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
        tracing::info!(
            "Forwarded UNSUBSCRIBE_NAMESPACE '{}' to session:{}",
            track_namespace_prefix,
            session_id
        );
        subscriber
            .send_unsubscribe_namespace(track_namespace_prefix)
            .await
    }
}

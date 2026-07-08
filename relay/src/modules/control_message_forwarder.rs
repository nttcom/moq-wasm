use std::sync::Arc;

use crate::modules::{
    core::{
        data_receiver::fetch_receiver::UpstreamFetchReceiver,
        data_sender::fetch_sender::FetchSender,
        handler::publish::SubscribeOption,
        subscription::UpstreamSubscription,
    },
    enums::{FilterType, GroupOrder},
    session_repository::SessionRepository,
    types::SessionId,
};

#[derive(Clone)]
pub(crate) struct ControlMessageForwarder {
    pub(crate) repository: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl ControlMessageForwarder {
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
        let publisher = self.repository.lock().await.publisher(session_id);
        if let Some(publisher) = publisher {
            match publisher
                .send_publish_namespace(track_namespace.to_string())
                .await
            {
                Ok(_) => true,
                Err(_) => {
                    tracing::error!("Failed to send publish namespace");
                    false
                }
            }
        } else {
            tracing::error!("No publisher");
            false
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
    ) -> bool {
        let publisher = self.repository.lock().await.publisher(session_id);
        if let Some(publisher) = publisher {
            match publisher
                .send_publish_namespace_done(track_namespace.to_string())
                .await
            {
                Ok(_) => true,
                Err(_) => {
                    tracing::error!("Failed to send publish namespace done");
                    false
                }
            }
        } else {
            tracing::error!("No publisher");
            false
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
    ) -> Option<u64> {
        let publisher = self.repository.lock().await.publisher(session_id);
        if let Some(publisher) = publisher {
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
        } else {
            tracing::error!("No publisher");
            None
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
        let subscriber = self.repository.lock().await.subscriber(session_id);
        let Some(mut subscriber) = subscriber else {
            tracing::error!("No subscriber");
            return Err(anyhow::anyhow!("No subscriber"));
        };
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
    ) -> anyhow::Result<(moqt::FetchHandle, Box<dyn UpstreamFetchReceiver>)> {
        let subscriber = self.repository.lock().await.subscriber(session_id);
        let Some(mut subscriber) = subscriber else {
            tracing::error!("No subscriber");
            return Err(anyhow::anyhow!("No subscriber"));
        };
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
                moqt::FetchOption::default(),
            )
            .await?;
        let receiver = subscriber.create_fetch_receiver(&handle).await?;
        Ok((handle, receiver))
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.control_message_forwarder.new_fetch_sender",
        skip_all,
        fields(session_id = %session_id, request_id = %request_id)
    )]
    pub(crate) async fn new_fetch_sender(
        &self,
        session_id: SessionId,
        request_id: u64,
    ) -> anyhow::Result<Box<dyn FetchSender>> {
        let publisher = self.repository.lock().await.publisher(session_id);
        let Some(publisher) = publisher else {
            tracing::error!("No publisher");
            return Err(anyhow::anyhow!("No publisher"));
        };
        publisher.new_fetch_sender(request_id).await
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
        let subscriber = self.repository.lock().await.subscriber(session_id);
        let Some(subscriber) = subscriber else {
            tracing::error!("No subscriber");
            return Err(anyhow::anyhow!("No subscriber"));
        };
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
        let subscriber = self.repository.lock().await.subscriber(session_id);
        let Some(subscriber) = subscriber else {
            tracing::error!("No subscriber");
            return Err(anyhow::anyhow!("No subscriber"));
        };
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

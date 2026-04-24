use std::sync::Arc;

use crate::modules::{
    core::{handler::publish::SubscribeOption, subscription::Subscription},
    enums::{FilterType, GroupOrder},
    session_repository::SessionRepository,
    types::SessionId,
};

pub(crate) struct Notifier {
    pub(crate) repository: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl Notifier {
    #[tracing::instrument(
        level = "info",
        name = "relay.notifier.publish_namespace",
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

    // pub(crate) fn subscribe_namespace(&self, session_id: String, namespace: String) {
    //     todo!()
    // }

    #[tracing::instrument(
        level = "info",
        name = "relay.notifier.publish",
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
        name = "relay.notifier.subscribe",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn subscribe(
        &self,
        session_id: SessionId,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<Subscription> {
        if let Some(mut subscriber) = self.repository.lock().await.subscriber(session_id) {
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
        } else {
            tracing::error!("No subscriber");
            Err(anyhow::anyhow!("No subscriber"))
        }
    }
}

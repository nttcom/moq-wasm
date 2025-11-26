use std::sync::Arc;

use uuid::Uuid;

use crate::modules::{
    core::{handler::publish::SubscribeOption, subscription::Subscription},
    enums::{FilterType, GroupOrder},
    repositories::session_repository::SessionRepository,
};

pub(crate) struct Notifier {
    pub(crate) repository: Arc<tokio::sync::Mutex<SessionRepository>>,
}

impl Notifier {
    pub(crate) async fn publish_namespace(
        &self,
        session_id: Uuid,
        track_namespace: String,
    ) -> bool {
        let publisher = self.repository.lock().await.get_publisher(session_id).await;
        if let Some(publisher) = publisher {
            match publisher
                .send_publish_namespace(track_namespace.to_string())
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "Sent publish namespace '{}' to {}",
                        track_namespace,
                        session_id
                    );
                    true
                }
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

    pub(crate) async fn publish(
        &self,
        session_id: Uuid,
        track_namespace: String,
        track_name: String,
        track_alias: u64,
    ) -> bool {
        let publisher = self.repository.lock().await.get_publisher(session_id).await;
        if let Some(publisher) = publisher {
            match publisher
                .send_publish(track_namespace.clone(), track_name, track_alias)
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "Sent publish namespace '{}' to {}",
                        track_namespace,
                        session_id
                    );
                    true
                }
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

    pub(crate) async fn subscribe(
        &self,
        session_id: Uuid,
        track_namespace: String,
        track_name: String,
    ) -> anyhow::Result<Box<dyn Subscription>> {
        if let Some(subscriber) = self
            .repository
            .lock()
            .await
            .get_subscriber(session_id)
            .await
        {
            let option = SubscribeOption {
                subscriber_priority: 128,
                group_order: GroupOrder::Ascending,
                forward: true,
                filter_type: FilterType::LatestObject,
            };
            subscriber
                .send_subscribe(track_namespace, track_name, option)
                .await
        } else {
            tracing::error!("No subscriber");
            Err(anyhow::anyhow!("No subscriber"))
        }
    }
}

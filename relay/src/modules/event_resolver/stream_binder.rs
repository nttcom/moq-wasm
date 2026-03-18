use std::sync::Arc;

use crate::modules::{
    core::{published_resource::PublishedResource, subscription::Subscription},
    event_resolver::stream_runner::StreamTaskRunner,
    relaies::{relay::Relay, relay_manager::RelayManager, relay_properties::RelayProperties},
    session_repository::SessionRepository,
    types::{SessionId, compose_session_track_key},
};

pub(crate) struct StreamBinder {
    session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
    relay_manager: Arc<RelayManager>,
    stream_runner: StreamTaskRunner,
}

impl StreamBinder {
    pub(crate) fn new(session_repo: Arc<tokio::sync::Mutex<SessionRepository>>) -> Self {
        Self {
            session_repo,
            relay_manager: Arc::new(RelayManager::new()),
            stream_runner: StreamTaskRunner::new(),
        }
    }

    pub(crate) async fn bind_by_subscribe(
        &self,
        subscriber_session_id: SessionId,
        subscription: Subscription,
        publisher_session_id: SessionId,
        published_resources: PublishedResource,
    ) {
        tracing::info!("bind by subscribe");
        let relay_manager = self.relay_manager.clone();
        let publisher = self
            .session_repo
            .lock()
            .await
            .publisher(subscriber_session_id)
            .await;
        let subscriber = self
            .session_repo
            .lock()
            .await
            .subscriber(publisher_session_id)
            .await;
        if publisher.is_none() || subscriber.is_none() {
            tracing::error!("Publisher or Subscriber session not found.");
            return;
        }
        let (publisher, subscriber) = (publisher.unwrap(), subscriber.unwrap());
        let task = async move {
            tracing::info!(
                "start relay task: track_alias={}",
                subscription.track_alias()
            );
            let receiver = match subscriber.create_data_receiver(subscription).await {
                Ok(receiver) => receiver,
                Err(_) => {
                    tracing::error!("Failed to accept data receiver");
                    return;
                }
            };
            tracing::debug!("accept type: {:?}", receiver);
            let prop = RelayProperties::new();
            let subscriber_track_alias = published_resources.track_alias();
            let sender = if receiver.datagram() {
                publisher.new_datagram(&published_resources)
            } else {
                match publisher
                    .new_stream(&published_resources, subscriber_track_alias)
                    .await
                {
                    Ok(sender) => sender,
                    Err(_) => {
                        tracing::error!("Failed to create stream sender");
                        return;
                    }
                }
            };
            let mut relay = Relay {
                relay_properties: prop,
            };
            let track_alias = receiver.get_track_alias();
            let track_key = compose_session_track_key(publisher_session_id, track_alias);
            relay.add_object_sender(
                publisher_session_id,
                track_alias,
                sender,
                published_resources.group_order(),
                published_resources.filter_type(),
            );
            relay.add_object_receiver(publisher_session_id, receiver);
            relay_manager.relay_map.insert(track_key, relay);
        };
        self.stream_runner.add_task(Box::pin(task)).await;
    }
}

use std::sync::Arc;

use crate::modules::{
    core::{
        published_resource::PublishedResource, publisher::Publisher, subscriber::Subscriber,
        subscription::Subscription,
    },
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
        let (publisher, mut subscriber) = (publisher.unwrap(), subscriber.unwrap());
        let task = async move {
            tracing::info!(
                "start relay task: track_alias={}",
                subscription.track_alias()
            );
            let receiver = match subscriber.create_data_receiver(&subscription).await {
                Ok(receiver) => receiver,
                Err(_) => {
                    tracing::error!("Failed to accept data receiver");
                    return;
                }
            };
            tracing::debug!("accept type: {:?}", receiver);
            let prop = RelayProperties::new();
            let subscriber_track_alias = published_resources.track_alias();
            let is_datagram = receiver.datagram();
            let sender = if is_datagram {
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
            relay.init_object_sender(
                publisher_session_id,
                track_alias,
                sender,
                published_resources.group_order(),
                published_resources.filter_type(),
            );
            relay.init_object_receiver(publisher_session_id, track_alias, receiver);
            relay_manager.relay_map.insert(track_key, relay);

            if is_datagram {
                tracing::info!("Datagram relay established for track_alias={}", track_alias);
                return;
            }

            Self::bind_stream(
                track_key,
                subscriber,
                subscription,
                publisher,
                published_resources,
                relay_manager,
            )
            .await;
        };
        self.stream_runner.add_task(Box::pin(task)).await;
    }

    async fn bind_stream(
        track_key: u128,
        mut subscriber: Box<dyn Subscriber>,
        subscription: Subscription,
        publisher: Box<dyn Publisher>,
        published_resources: PublishedResource,
        relay_manager: Arc<RelayManager>,
    ) {
        loop {
            let receiver = match subscriber.create_data_receiver(&subscription).await {
                Ok(receiver) => receiver,
                Err(_) => {
                    tracing::error!("Failed to accept data receiver");
                    return;
                }
            };
            if receiver.datagram() {
                tracing::error!("Expected stream receiver but got datagram receiver");
                return;
            }

            let subscriber_track_alias = receiver.get_track_alias();

            let sender = match publisher
                .new_stream(&published_resources, subscriber_track_alias)
                .await
            {
                Ok(sender) => sender,
                Err(_) => {
                    tracing::error!("Failed to create stream sender");
                    return;
                }
            };
            if let Some(mut relay) = relay_manager.relay_map.get_mut(&track_key) {
                relay.add_object_sender(
                    track_key,
                    sender,
                    published_resources.group_order(),
                    published_resources.filter_type(),
                );
                relay.add_object_receiver(track_key, receiver);
            }
        }
    }
}

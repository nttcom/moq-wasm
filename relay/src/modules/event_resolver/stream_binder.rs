use std::sync::Arc;

use crate::modules::{
    core::{published_resource::PublishedResource, subscription::Subscription},
    event_resolver::stream_runner::StreamTaskRunner,
    relaies::{relay::Relay, relay_manager::RelayManager, relay_properties::RelayProperties},
};

pub(crate) struct StreamBinder {
    relay_manager: Arc<RelayManager>,
    stream_runner: StreamTaskRunner,
}

impl StreamBinder {
    pub(crate) fn new() -> Self {
        Self {
            relay_manager: Arc::new(RelayManager::new()),
            stream_runner: StreamTaskRunner::new(),
        }
    }

    pub(crate) async fn bind_by_subscribe(
        &self,
        subscription: Box<dyn Subscription>,
        published_resources: Box<dyn PublishedResource>,
    ) {
        tracing::info!("bind by subscribe");
        let relay_manager = self.relay_manager.clone();
        let task = async move {
            let receiver = match subscription.create_data_receiver().await {
                Ok(receiver) => receiver,
                Err(_) => {
                    tracing::error!("Failed to accept data receiver");
                    return;
                }
            };
            tracing::debug!("accept type: {:?}", receiver);
            let prop = RelayProperties::new();
            let sender = if receiver.is_datagram() {
                published_resources.new_datagram()
            } else {
                match published_resources.new_stream().await {
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
            let track_alias = receiver.track_alias();
            relay.add_object_receiver(receiver);
            relay.add_object_sender(
                track_alias,
                sender,
                published_resources.group_order(),
                published_resources.filter_type(),
            );
            relay_manager.relay_map.insert(track_alias, relay);
        };
        self.stream_runner.add_task(Box::pin(task)).await;
    }
}

use std::sync::Arc;

use crate::modules::{
    core::{
        publish_resource::PublishedResource,
        subscription::{Acceptance, Subscription},
    },
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

    pub(crate) fn forward_true_publish(&self) {}

    pub(crate) fn subscribe_for_forward_true_publication() {}

    pub(crate) async fn bind_by_subscribe(
        &self,
        subscription: Box<dyn Subscription>,
        publication: Box<dyn PublishedResource>,
    ) {
        tracing::info!("bind by subscribe");
        let relay_manager = self.relay_manager.clone();
        let task = async move {
            let acceptance = match subscription.accept_stream_or_datagram().await {
                Ok(acceptance) => acceptance,
                Err(e) => {
                    tracing::error!("Failed to accept stream or datagram");
                    return;
                }
            };
            tracing::debug!("accepct type: {:?}", acceptance);
            let prop = RelayProperties::new();
            if let Acceptance::Datagram(receiver, object) = acceptance {
                tracing::info!("bind by datagram");
                let datagram_sender = publication.create_datagram();
                let mut relay = Relay {
                    relay_properties: prop,
                };
                let track_alias = receiver.track_alias();
                relay.add_object_receiver(receiver, object);
                relay.add_object_sender(track_alias, datagram_sender);
                relay_manager.relay_map.insert(track_alias, relay);
            }
        };
        self.stream_runner.add_task(Box::pin(task)).await;
    }
}

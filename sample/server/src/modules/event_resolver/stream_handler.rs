use dashmap::DashMap;

use crate::modules::{
    core::{
        publication::Publication,
        subscription::{Acceptance, Subscription},
    },
    relaies::{relay::Relay, relay_manager::RelayManager, relay_properties::RelayProperties},
};

struct Binder {
    relay_manager: RelayManager,
}

impl Binder {
    pub(crate) fn new() -> Self {
        Self {
            relay_manager: RelayManager {
                relay_map: DashMap::new(),
            },
        }
    }

    pub(crate) fn forward_true_publish(&self) {}

    pub(crate) fn subscribe_for_forward_true_publication() {}

    pub(crate) async fn bind_by_subscribe(
        &self,
        subscription: Box<dyn Subscription>,
        publication: Box<dyn Publication>,
    ) -> anyhow::Result<()> {
        let acceptance = subscription.accept_stream_or_datagram().await?;
        let prop = RelayProperties::new();
        if let Acceptance::Datagram(receiver, object) = acceptance {
            let datagram_sender = publication.create_datagram();
            let mut relay = Relay {
                relay_properties: prop,
            };
            let track_alias = receiver.track_alias();
            relay.add_object_receiver(receiver, object);
            relay.add_object_sender(track_alias, datagram_sender);
        }
        Ok(())
    }
}

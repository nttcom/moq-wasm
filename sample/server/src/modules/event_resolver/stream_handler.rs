use dashmap::DashMap;

use crate::modules::{
    core::{
        publication::Publication,
        subscription::{Acceptance, Subscription},
    },
    relaies::relay_manager::RelayManager,
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
        if let Acceptance::Datagram(receiver, object) = acceptance {
            // let datagram_sender = publication.create_datagram();
            // let relay = Relay;
        }
        Ok(())
    }
}

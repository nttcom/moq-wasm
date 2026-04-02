use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use crate::modules::{
    core::{published_resource::PublishedResource, publisher::Publisher},
    relay::{
        cache::track_cache::TrackCache,
        caches::{delivery_type_map::DeliveryTypeMap, latest_info::LatestInfo},
    },
    types::TrackKey,
};

use super::{group_sender::GroupSender, scheduler::EgressScheduler};

pub(crate) struct EgressRunner {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<LatestInfo>,
    delivery_type_map: Arc<DeliveryTypeMap>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<LatestInfo>,
        delivery_type_map: Arc<DeliveryTypeMap>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
    ) -> Self {
        Self {
            track_key,
            cache,
            latest_info_sender,
            delivery_type_map,
            publisher,
            published_resource,
        }
    }

    pub(crate) async fn run(self) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel(64);

        let filter_type = self.published_resource.filter_type();
        let scheduler = EgressScheduler::new(
            self.track_key,
            self.cache.clone(),
            self.latest_info_sender,
            self.delivery_type_map,
            filter_type,
            tx,
        );
        let group_sender =
            GroupSender::new(self.cache, self.publisher, self.published_resource, rx);

        tokio::join!(scheduler.run(), group_sender.run());
        Ok(())
    }
}

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};

use crate::modules::{
    core::{published_resource::PublishedResource, publisher::Publisher},
    relay::{cache::track_cache::TrackCache, notifications::latest_info::LatestInfo},
};

use super::{group_sender::GroupSender, scheduler::EgressScheduler};

pub(crate) struct EgressRunner {
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<LatestInfo>,
    publisher: Box<dyn Publisher>,
    published_resource: PublishedResource,
}

impl EgressRunner {
    pub(crate) fn new(
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<LatestInfo>,
        publisher: Box<dyn Publisher>,
        published_resource: PublishedResource,
    ) -> Self {
        Self {
            cache,
            latest_info_sender,
            publisher,
            published_resource,
        }
    }

    pub(crate) async fn run(self) -> anyhow::Result<()> {
        let (sender, receiver) = mpsc::channel(64);

        let filter_type = self.published_resource.filter_type();
        let group_order = self.published_resource.group_order();
        let scheduler = EgressScheduler::new(
            self.cache.clone(),
            self.latest_info_sender,
            filter_type,
            group_order,
            sender,
        );
        let group_sender = GroupSender::new(
            self.cache,
            self.publisher,
            self.published_resource,
            receiver,
        );

        tokio::join!(scheduler.run(), group_sender.run());
        Ok(())
    }
}

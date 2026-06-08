use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::modules::{
    core::{publisher::Publisher, subscription::DownstreamSubscription},
    relay::{cache::track_cache::TrackCache, notifications::track_event::TrackEvent},
    types::TrackKey,
};

use super::{group_sender::GroupSender, scheduler::EgressScheduler};

pub(crate) struct EgressRunner {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    latest_info_sender: broadcast::Sender<TrackEvent>,
    publisher: Box<dyn Publisher>,
    downstream_subscription: DownstreamSubscription,
    ready_sender: oneshot::Sender<anyhow::Result<()>>,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<TrackEvent>,
        publisher: Box<dyn Publisher>,
        downstream_subscription: DownstreamSubscription,
        ready_sender: oneshot::Sender<anyhow::Result<()>>,
    ) -> Self {
        Self {
            track_key,
            cache,
            latest_info_sender,
            publisher,
            downstream_subscription,
            ready_sender,
        }
    }

    pub(crate) async fn run(self) -> anyhow::Result<()> {
        let (sender, receiver) = mpsc::channel(64);

        let filter_type = self.downstream_subscription.filter_type();
        let group_order = self.downstream_subscription.group_order();
        let scheduler = EgressScheduler::new(
            self.cache.clone(),
            self.latest_info_sender,
            filter_type,
            group_order,
            sender,
            self.ready_sender,
        );
        let group_sender = GroupSender::new(
            self.track_key,
            self.cache,
            self.publisher,
            self.downstream_subscription,
            receiver,
        );

        tokio::join!(scheduler.run(), group_sender.run());
        Ok(())
    }
}

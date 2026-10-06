use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::{mpsc, oneshot, watch};

use crate::modules::{
    core::{publisher::Publisher, subscription::DownstreamSubscription},
    data_plane::cache::track_cache::TrackCache,
    sequences::tables::table::PublishDoneReason,
    types::TrackKey,
};

use super::{group_sender::GroupSender, scheduler::EgressScheduler};

pub(crate) struct EgressRunner {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    publisher: Arc<dyn Publisher>,
    downstream_subscription: DownstreamSubscription,
    ready_sender: oneshot::Sender<anyhow::Result<()>>,
    largest_location: Option<moqt::Location>,
    forward_receiver: watch::Receiver<bool>,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        publisher: Box<dyn Publisher>,
        downstream_subscription: DownstreamSubscription,
        ready_sender: oneshot::Sender<anyhow::Result<()>>,
        largest_location: Option<moqt::Location>,
        forward_receiver: watch::Receiver<bool>,
    ) -> Self {
        Self {
            track_key,
            cache,
            publisher: Arc::from(publisher),
            downstream_subscription,
            ready_sender,
            largest_location,
            forward_receiver,
        }
    }

    pub(crate) async fn run(
        self,
        stop_receiver: oneshot::Receiver<PublishDoneReason>,
        subscribe_ok_receiver: oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        let track_key = self.track_key.clone();
        let publisher = self.publisher.clone();
        let request_id = self.downstream_subscription.request_id();
        let opened_stream_count = Arc::new(AtomicU64::new(0));
        let delivery = self.deliver(opened_stream_count.clone());

        let reason = tokio::select! {
            biased;
            reason = stop_receiver => {
                tracing::debug!("downstream subscription removed; egress runner stopped");
                reason.ok()
            }
            reason = delivery => reason,
        };
        // PUBLISH_DONE must follow the SUBSCRIBE_OK the subscriber's session worker sends; a
        // subscription that never got one is answered with SUBSCRIBE_ERROR instead.
        if let Some(reason) = reason
            && subscribe_ok_receiver.await.is_ok()
        {
            Self::send_publish_done(
                publisher.as_ref(),
                &track_key,
                request_id,
                reason,
                opened_stream_count.load(Ordering::Acquire),
            )
            .await;
        }
        Ok(())
    }

    async fn deliver(self, opened_stream_count: Arc<AtomicU64>) -> Option<PublishDoneReason> {
        let Self {
            track_key,
            cache,
            publisher,
            downstream_subscription,
            ready_sender,
            largest_location,
            forward_receiver,
        } = self;

        if cache.is_malformed() {
            let _ = ready_sender.send(Ok(()));
            return Some(Self::malformed(&track_key));
        }

        let (sender, receiver) = mpsc::channel(64);
        let filter_type = downstream_subscription.filter_type();
        let group_order = downstream_subscription.group_order();
        let scheduler = EgressScheduler::new(
            cache.clone(),
            filter_type,
            group_order,
            sender,
            largest_location,
            forward_receiver,
        );
        let group_sender = GroupSender::new(
            track_key.clone(),
            cache.clone(),
            publisher,
            downstream_subscription,
            receiver,
            opened_stream_count,
        );

        tokio::select! {
            _ = async { tokio::join!(scheduler.run(ready_sender), group_sender.run()) } => None,
            _ = cache.malformed_track_detected() => Some(Self::malformed(&track_key)),
        }
    }

    fn malformed(track_key: &TrackKey) -> PublishDoneReason {
        tracing::warn!(%track_key, "malformed track detected; terminating downstream subscription");
        PublishDoneReason::malformed_track()
    }

    async fn send_publish_done(
        publisher: &dyn Publisher,
        track_key: &TrackKey,
        request_id: u64,
        reason: PublishDoneReason,
        stream_count: u64,
    ) {
        if let Err(error) = publisher
            .send_publish_done(
                request_id,
                reason.status_code,
                stream_count,
                reason.error_reason,
            )
            .await
        {
            tracing::error!(
                ?error,
                %track_key,
                request_id,
                "failed to send PUBLISH_DONE"
            );
        }
    }
}

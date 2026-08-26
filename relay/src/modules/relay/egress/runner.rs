use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tokio::sync::{broadcast, mpsc, oneshot};

use crate::modules::{
    core::{publisher::Publisher, subscription::DownstreamSubscription},
    enums::PublishDoneStatusCode,
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
    largest_location: Option<moqt::Location>,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        latest_info_sender: broadcast::Sender<TrackEvent>,
        publisher: Box<dyn Publisher>,
        downstream_subscription: DownstreamSubscription,
        ready_sender: oneshot::Sender<anyhow::Result<()>>,
        largest_location: Option<moqt::Location>,
    ) -> Self {
        Self {
            track_key,
            cache,
            latest_info_sender,
            publisher,
            downstream_subscription,
            ready_sender,
            largest_location,
        }
    }

    pub(crate) async fn run(self) -> anyhow::Result<()> {
        let publisher: Arc<dyn Publisher> = Arc::from(self.publisher);
        let request_id = self.downstream_subscription.request_id();

        // The subscribe sequence rejects known-malformed tracks, but detection
        // can race it. Answer readiness (so SUBSCRIBE_OK goes out) and
        // terminate immediately; no streams were opened yet (Stream Count 0).
        if self.cache.is_malformed() {
            let _ = self.ready_sender.send(Ok(()));
            Self::send_malformed_publish_done(publisher.as_ref(), &self.track_key, request_id, 0)
                .await;
            return Ok(());
        }

        let (sender, receiver) = mpsc::channel(64);
        let opened_stream_count = Arc::new(AtomicU64::new(0));
        let filter_type = self.downstream_subscription.filter_type();
        let group_order = self.downstream_subscription.group_order();
        let scheduler = EgressScheduler::new(
            self.cache.clone(),
            self.latest_info_sender,
            filter_type,
            group_order,
            sender,
            self.ready_sender,
            self.largest_location,
        );
        let group_sender = GroupSender::new(
            self.track_key.clone(),
            self.cache.clone(),
            publisher.clone(),
            self.downstream_subscription,
            receiver,
            opened_stream_count.clone(),
        );

        tokio::select! {
            _ = async { tokio::join!(scheduler.run(), group_sender.run()) } => {}
            _ = self.cache.malformed_track_detected() => {
                // §2.5: a relay MUST immediately terminate downstream
                // subscriptions of a malformed track with PUBLISH_DONE.
                // Dropping the scheduler and sender futures above aborts every
                // per-group send task, closing all data streams before the
                // control message goes out (§9.12 requires that ordering).
                // The count is taken at stream-open time, so it is exact even
                // though the send tasks were just aborted (a datagram track
                // stays at 0, as §9.12 requires).
                let stream_count = opened_stream_count.load(Ordering::Acquire);
                Self::send_malformed_publish_done(
                    publisher.as_ref(),
                    &self.track_key,
                    request_id,
                    stream_count,
                )
                .await;
            }
        }
        Ok(())
    }

    async fn send_malformed_publish_done(
        publisher: &dyn Publisher,
        track_key: &TrackKey,
        request_id: u64,
        stream_count: u64,
    ) {
        tracing::warn!(
            %track_key,
            request_id,
            "malformed track detected; terminating downstream subscription"
        );
        if let Err(error) = publisher
            .send_publish_done(
                request_id,
                PublishDoneStatusCode::MalformedTrack as u64,
                stream_count,
                "malformed track".to_string(),
            )
            .await
        {
            tracing::error!(
                ?error,
                %track_key,
                request_id,
                "failed to send PUBLISH_DONE for malformed track"
            );
        }
    }
}

use std::sync::Arc;

use moqt::publish::PublishDoneCode;
use tokio::sync::{mpsc, oneshot, watch};

use crate::modules::{
    data_plane::cache::track_cache::TrackCache,
    domain::{
        delivery_control::DeliveryControl,
        delivery_stats::DeliveryStats,
        pub_sub_directory::{DownstreamRunnerSignals, entry::PublishDoneReason},
        track_key::TrackKey,
    },
    session::{publisher::Publisher, subscription::DownstreamSubscription},
};

use super::{
    group_sender::GroupSender,
    scheduler::{EgressScheduler, SchedulerExit},
};

pub(crate) enum RunnerExit {
    /// The runner delivered the whole range of its subscription and sent
    /// PUBLISH_DONE; its registration is still to be removed.
    SubscriptionEnded,
    /// The registration was removed, or the track ended delivery.
    Stopped,
}

pub(crate) struct EgressRunner {
    stop_receiver: oneshot::Receiver<PublishDoneReason>,
    delivery: Delivery,
}

struct Delivery {
    track_key: TrackKey,
    cache: Arc<TrackCache>,
    publisher: Arc<dyn Publisher>,
    downstream_subscription: DownstreamSubscription,
    ready_sender: oneshot::Sender<anyhow::Result<()>>,
    largest_location: Option<moqt::object::Location>,
    delivery_control_receiver: watch::Receiver<DeliveryControl>,
    delivery_stats: Arc<DeliveryStats>,
}

impl EgressRunner {
    pub(crate) fn new(
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        publisher: Box<dyn Publisher>,
        downstream_subscription: DownstreamSubscription,
        ready_sender: oneshot::Sender<anyhow::Result<()>>,
        largest_location: Option<moqt::object::Location>,
        signals: DownstreamRunnerSignals,
    ) -> Self {
        let DownstreamRunnerSignals {
            stop_receiver,
            delivery_control_receiver,
            delivery_stats,
        } = signals;
        Self {
            stop_receiver,
            delivery: Delivery {
                track_key,
                cache,
                publisher: Arc::from(publisher),
                downstream_subscription,
                ready_sender,
                largest_location,
                delivery_control_receiver,
                delivery_stats,
            },
        }
    }

    pub(crate) async fn run(self, subscribe_ok_receiver: oneshot::Receiver<()>) -> RunnerExit {
        let Self {
            stop_receiver,
            delivery,
        } = self;
        let track_key = delivery.track_key.clone();
        let publisher = delivery.publisher.clone();
        let downstream_subscription = delivery.downstream_subscription.clone();
        let delivery_stats = delivery.delivery_stats.clone();

        let (reason, exit) = tokio::select! {
            biased;
            reason = stop_receiver => {
                tracing::debug!("downstream subscription removed; egress runner stopped");
                (reason.ok(), RunnerExit::Stopped)
            }
            reason = delivery.run() => {
                let exit = match &reason {
                    Some(reason) if reason.status_code == PublishDoneCode::SubscriptionEnded => {
                        tracing::info!("downstream subscription reached its End Group");
                        RunnerExit::SubscriptionEnded
                    }
                    _ => RunnerExit::Stopped,
                };
                (reason, exit)
            }
        };
        // PUBLISH_DONE must follow the SUBSCRIBE_OK the subscriber's session worker sends; a
        // subscription that never got one is answered with SUBSCRIBE_ERROR instead.
        if let Some(reason) = reason
            && subscribe_ok_receiver.await.is_ok()
        {
            Self::send_publish_done(
                publisher.as_ref(),
                &track_key,
                &downstream_subscription,
                reason,
                delivery_stats.streams_opened(),
            )
            .await;
        }
        exit
    }
    fn malformed(track_key: &TrackKey) -> PublishDoneReason {
        tracing::warn!(%track_key, "malformed track detected; terminating downstream subscription");
        PublishDoneReason::malformed_track()
    }

    async fn send_publish_done(
        publisher: &dyn Publisher,
        track_key: &TrackKey,
        subscription: &DownstreamSubscription,
        reason: PublishDoneReason,
        stream_count: u64,
    ) {
        if let Err(error) = publisher
            .send_publish_done(
                subscription,
                reason.status_code,
                stream_count,
                reason.error_reason,
            )
            .await
        {
            tracing::error!(
                ?error,
                %track_key,
                request_id = subscription.request_id(),
                "failed to send PUBLISH_DONE"
            );
        }
    }
}

impl Delivery {
    async fn run(self) -> Option<PublishDoneReason> {
        let Self {
            track_key,
            cache,
            publisher,
            downstream_subscription,
            ready_sender,
            largest_location,
            delivery_control_receiver,
            delivery_stats,
        } = self;

        if cache.is_malformed() {
            let _ = ready_sender.send(Ok(()));
            return Some(EgressRunner::malformed(&track_key));
        }

        let (sender, receiver) = mpsc::channel(64);
        let group_order = downstream_subscription.group_order();
        let scheduler = EgressScheduler::new(
            cache.clone(),
            group_order,
            sender,
            largest_location,
            delivery_control_receiver.clone(),
        );
        let group_sender = GroupSender::new(
            track_key.clone(),
            cache.clone(),
            publisher,
            downstream_subscription,
            receiver,
            delivery_stats,
            delivery_control_receiver,
        );

        tokio::select! {
            (exit, ()) = async { tokio::join!(scheduler.run(ready_sender), group_sender.run()) } => {
                // The scheduler dropping its task sender lets the group sender
                // finish only after every stream it opened has been closed or
                // reset (draft-14 §9.12).
                match exit {
                    SchedulerExit::EndGroupReached => Some(PublishDoneReason::subscription_ended()),
                    SchedulerExit::OpenEventsClosed => None,
                }
            }
            _ = cache.malformed_track_detected() => Some(EgressRunner::malformed(&track_key)),
        }
    }
}

use std::sync::Arc;

use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::{Instrument, Span};

use crate::logging::{link_span, relay_hostname};
use crate::modules::{
    data_plane::{
        cache::track_cache::TrackCache,
        egress::runner::{EgressRunner, RunnerExit},
    },
    domain::{runner_signals::DownstreamRunnerSignals, session_id::SessionId, track_key::TrackKey},
    session::{
        publisher::Publisher, session_event::SessionEvent, subscription::DownstreamSubscription,
    },
};

pub(crate) struct EgressStart {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) downstream_subscription: DownstreamSubscription,
    pub(crate) publisher: Box<dyn Publisher>,
    pub(crate) subscriber_mailbox_sender: mpsc::UnboundedSender<SessionEvent>,
    pub(crate) cache: Arc<TrackCache>,
    /// The request that starts the egress; the egress outlives it and links to it.
    pub(crate) parent_span: Span,
    pub(crate) ready_sender: oneshot::Sender<anyhow::Result<()>>,
    pub(crate) runner_signals: DownstreamRunnerSignals,
    pub(crate) subscribe_ok_receiver: oneshot::Receiver<()>,
    /// From LargestLocation of SUBSCRIBE_OK; `None` when no content has been
    /// delivered yet.
    pub(crate) largest_location: Option<moqt::object::Location>,
}

/// The egress runner of one downstream subscription. The consumer's stop
/// sender ends it; a runner that ends the subscription itself sends
/// `DownstreamSubscriptionEnded` to the subscriber session's mailbox, whose
/// worker then removes the consumer.
pub(crate) struct EgressTask {
    _join_handle: JoinHandle<()>,
}

impl EgressTask {
    pub(crate) fn run(start: EgressStart) -> Self {
        let track_alias = start.downstream_subscription.track_alias();
        // The track outlives the request that starts it, so its span roots its
        // own trace, linked to the request.
        let egress_track_span = tracing::info_span!(
            parent: None,
            "relay.dataplane.egress.track",
            relay_hostname = relay_hostname(),
            subscriber_session_id = %start.subscriber_session_id,
            request_id = start.request_id,
            track_alias = track_alias,
            track_namespace = %start.track_key.track_namespace,
            track_name = %start.track_key.track_name,
        );
        link_span(&egress_track_span, &start.parent_span);

        let runner = EgressRunner::new(
            start.track_key,
            start.cache,
            start.publisher,
            start.downstream_subscription,
            start.ready_sender,
            start.largest_location,
            start.runner_signals,
        );
        let subscribe_ok_receiver = start.subscribe_ok_receiver;
        let subscriber_mailbox_sender = start.subscriber_mailbox_sender;
        let subscription_ended = SessionEvent::DownstreamSubscriptionEnded {
            request_id: start.request_id,
        };
        let join_handle = tokio::spawn(
            async move {
                if let RunnerExit::SubscriptionEnded = runner.run(subscribe_ok_receiver).await {
                    let _ = subscriber_mailbox_sender.send(subscription_ended);
                }
            }
            .instrument(egress_track_span),
        );
        Self {
            _join_handle: join_handle,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::watch;

    use super::*;
    use crate::modules::{
        domain::{delivery_control::DeliveryControl, publish_done_reason::PublishDoneReason},
        test_support::{
            directory_fixtures::delivery_control,
            relay_harness::{
                MockPublisher, MockPublisherObservers, fixtures::subscription::make_subscription,
            },
        },
    };

    const SUBSCRIBER_SESSION_ID: SessionId = 2;
    const DOWNSTREAM_SUBSCRIBE_ID: u64 = 7;
    const TEST_TIMEOUT: Duration = Duration::from_secs(3);
    const PUBLISH_DONE_WINDOW: Duration = Duration::from_millis(100);

    struct TestContext {
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        idle_cache_reference_count: usize,
        publisher: MockPublisher,
        observers: MockPublisherObservers,
        subscriber_mailbox_sender: mpsc::UnboundedSender<SessionEvent>,
        subscriber_mailbox_receiver: mpsc::UnboundedReceiver<SessionEvent>,
    }

    fn setup() -> TestContext {
        let (publisher, observers) = MockPublisher::channel();
        let cache = Arc::new(TrackCache::new());
        let idle_cache_reference_count = Arc::strong_count(&cache);
        let (subscriber_mailbox_sender, subscriber_mailbox_receiver) = mpsc::unbounded_channel();
        TestContext {
            track_key: TrackKey::new("ns", "track"),
            cache,
            idle_cache_reference_count,
            publisher,
            observers,
            subscriber_mailbox_sender,
            subscriber_mailbox_receiver,
        }
    }

    async fn start_reader_without_subscribe_ok(
        ctx: &TestContext,
        runner_stop_receiver: oneshot::Receiver<PublishDoneReason>,
        control: DeliveryControl,
    ) -> (
        Result<anyhow::Result<()>, oneshot::error::RecvError>,
        oneshot::Sender<()>,
    ) {
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (subscribe_ok_sender, subscribe_ok_receiver) = oneshot::channel();
        let _egress = EgressTask::run(EgressStart {
            subscriber_session_id: SUBSCRIBER_SESSION_ID,
            request_id: DOWNSTREAM_SUBSCRIBE_ID,
            track_key: ctx.track_key.clone(),
            downstream_subscription: make_subscription(moqt::subscribe::FilterType::LargestObject),
            publisher: Box::new(ctx.publisher.clone()),
            subscriber_mailbox_sender: ctx.subscriber_mailbox_sender.clone(),
            cache: ctx.cache.clone(),
            parent_span: Span::none(),
            ready_sender,
            runner_signals: DownstreamRunnerSignals {
                stop_receiver: runner_stop_receiver,
                delivery_control_receiver: watch::channel(control).1,
                delivery_stats: Arc::default(),
            },
            subscribe_ok_receiver,
            largest_location: None,
        });
        let readiness = tokio::time::timeout(TEST_TIMEOUT, ready_receiver)
            .await
            .expect("runner should resolve its readiness");
        (readiness, subscribe_ok_sender)
    }

    async fn start_reader(
        ctx: &TestContext,
        runner_stop_receiver: oneshot::Receiver<PublishDoneReason>,
    ) -> Result<anyhow::Result<()>, oneshot::error::RecvError> {
        let (readiness, subscribe_ok_sender) =
            start_reader_without_subscribe_ok(ctx, runner_stop_receiver, delivery_control(true))
                .await;
        if matches!(readiness, Ok(Ok(()))) {
            let _ = subscribe_ok_sender.send(());
        }
        readiness
    }

    async fn assert_no_publish_done_within_window(ctx: &mut TestContext) {
        let publish_done =
            tokio::time::timeout(PUBLISH_DONE_WINDOW, ctx.observers.publish_done.recv()).await;
        assert!(publish_done.is_err(), "no PUBLISH_DONE should be sent");
    }

    fn runner_holds_cache(ctx: &TestContext) -> bool {
        Arc::strong_count(&ctx.cache) > ctx.idle_cache_reference_count
    }

    async fn wait_until_runner_released_cache(ctx: &TestContext) {
        tokio::time::timeout(TEST_TIMEOUT, async {
            while runner_holds_cache(ctx) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("egress runner should stop and release the track cache");
    }

    #[tokio::test]
    async fn runner_whose_subscription_was_removed_before_start_never_runs() {
        // Arrange
        let ctx = setup();
        let (runner_stop_sender, runner_stop_receiver) = oneshot::channel();
        drop(runner_stop_sender);

        // Act
        let readiness = start_reader(&ctx, runner_stop_receiver).await;

        // Assert
        assert!(readiness.is_err());
        wait_until_runner_released_cache(&ctx).await;
    }

    #[tokio::test]
    async fn runner_whose_upstream_ended_before_start_sends_no_publish_done() {
        // Arrange
        let mut ctx = setup();
        let (runner_stop_sender, runner_stop_receiver) = oneshot::channel();
        runner_stop_sender
            .send(PublishDoneReason::publisher_session_closed())
            .unwrap();

        // Act
        let readiness = start_reader(&ctx, runner_stop_receiver).await;

        // Assert
        assert!(readiness.is_err());
        assert_no_publish_done_within_window(&mut ctx).await;
    }

    #[tokio::test]
    async fn publish_done_waits_for_subscribe_ok() {
        // Arrange
        let mut ctx = setup();
        let (runner_stop_sender, runner_stop_receiver) = oneshot::channel();
        let (readiness, subscribe_ok_sender) =
            start_reader_without_subscribe_ok(&ctx, runner_stop_receiver, delivery_control(true))
                .await;
        readiness
            .expect("runner readiness should not be dropped")
            .expect("runner should start");
        runner_stop_sender
            .send(PublishDoneReason::publisher_session_closed())
            .unwrap();
        assert_no_publish_done_within_window(&mut ctx).await;

        // Act
        subscribe_ok_sender.send(()).unwrap();

        // Assert
        tokio::time::timeout(TEST_TIMEOUT, ctx.observers.publish_done.recv())
            .await
            .expect("runner should send PUBLISH_DONE")
            .expect("runner dropped its publisher");
    }

    #[tokio::test]
    async fn runner_stops_once_its_subscription_is_removed() {
        // Arrange
        let mut ctx = setup();
        let (runner_stop_sender, runner_stop_receiver) = oneshot::channel();
        start_reader(&ctx, runner_stop_receiver)
            .await
            .expect("runner readiness should not be dropped")
            .expect("runner should start");
        assert!(runner_holds_cache(&ctx));

        // Act
        drop(runner_stop_sender);

        // Assert
        wait_until_runner_released_cache(&ctx).await;
        assert_no_publish_done_within_window(&mut ctx).await;
    }

    #[tokio::test]
    async fn runner_that_delivered_its_range_reports_the_subscription_ended() {
        // Arrange: a start past the End Group leaves nothing to deliver
        let mut ctx = setup();
        let (_runner_stop_sender, runner_stop_receiver) = oneshot::channel();
        let empty_range = DeliveryControl::from_subscribe_update(
            true,
            moqt::object::Location {
                group_id: 1,
                object_id: 0,
            },
            1,
        );

        // Act
        let (readiness, subscribe_ok_sender) =
            start_reader_without_subscribe_ok(&ctx, runner_stop_receiver, empty_range).await;
        readiness
            .expect("runner readiness should not be dropped")
            .expect("runner should start");
        subscribe_ok_sender.send(()).unwrap();

        // Assert
        let publish_done = tokio::time::timeout(TEST_TIMEOUT, ctx.observers.publish_done.recv())
            .await
            .expect("runner should send PUBLISH_DONE")
            .expect("runner dropped its publisher");
        assert_eq!(
            publish_done.status_code,
            moqt::publish::PublishDoneCode::SubscriptionEnded
        );
        assert_eq!(publish_done.stream_count, 0);
        let event = tokio::time::timeout(TEST_TIMEOUT, ctx.subscriber_mailbox_receiver.recv())
            .await
            .expect("runner should report the ended subscription to the subscriber session")
            .expect("the subscriber mailbox should stay open");
        assert!(matches!(
            event,
            SessionEvent::DownstreamSubscriptionEnded {
                request_id: DOWNSTREAM_SUBSCRIBE_ID
            }
        ));
    }
}

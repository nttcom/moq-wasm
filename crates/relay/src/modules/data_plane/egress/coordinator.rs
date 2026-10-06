use std::sync::Arc;

use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::{
        cache::{store::TrackCacheStore, track_cache::TrackCache},
        egress::{fetch_delivery::deliver_fetch, runner::EgressRunner},
    },
    domain::{
        pub_sub_directory::entry::PublishDoneReason, session_id::SessionId, track_key::TrackKey,
    },
    session::{session_repository::SessionRepository, subscription::DownstreamSubscription},
};

pub(crate) struct EgressStartRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) downstream_subscribe_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) downstream_subscription: DownstreamSubscription,
    pub(crate) parent_span: Span,
    pub(crate) ready_sender: oneshot::Sender<anyhow::Result<()>>,
    pub(crate) runner_stop_receiver: oneshot::Receiver<PublishDoneReason>,
    pub(crate) subscribe_ok_receiver: oneshot::Receiver<()>,
    pub(crate) forward_receiver: watch::Receiver<bool>,
    /// From LargestLocation of SUBSCRIBE_OK
    /// None means that no content has been delivered yet.
    pub(crate) largest_location: Option<moqt::Location>,
}

pub(crate) struct EgressFetchRequest {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) start_location: moqt::Location,
    pub(crate) end_location: moqt::Location,
    pub(crate) group_order: moqt::GroupOrder,
}

pub(crate) enum EgressCommand {
    StartReader(Box<EgressStartRequest>),
    StartFetch(EgressFetchRequest),
}

pub(crate) struct EgressCoordinator {
    command_sender: mpsc::Sender<EgressCommand>,
    command_runner: tokio::task::JoinHandle<()>,
}

impl EgressCoordinator {
    pub(crate) fn new(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
    ) -> Self {
        let (command_sender, mut command_receiver) = mpsc::channel::<EgressCommand>(512);

        let command_runner = tokio::spawn(async move {
            let mut runners = JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = runners.join_next() => {}
                    command = command_receiver.recv() => {
                        let Some(command) = command else {
                            break;
                        };
                        match command {
                            EgressCommand::StartReader(request) => {
                                Self::spawn_runner(
                                    &mut runners,
                                    session_repo.clone(),
                                    cache_store.clone(),
                                    *request,
                                )
                                .await;
                            }
                            EgressCommand::StartFetch(request) => {
                                Self::spawn_fetch_delivery(session_repo.clone(), request).await;
                            }
                        }
                    }
                }
            }
        });

        Self {
            command_sender,
            command_runner,
        }
    }

    pub(crate) fn sender(&self) -> mpsc::Sender<EgressCommand> {
        self.command_sender.clone()
    }

    async fn spawn_fetch_delivery(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        request: EgressFetchRequest,
    ) {
        let fetching_session_publisher = session_repo
            .lock()
            .await
            .publisher(request.subscriber_session_id);
        let Some(publisher) = fetching_session_publisher else {
            tracing::error!("session not found for fetch");
            return;
        };

        tokio::spawn(async move {
            let sender = match publisher.new_fetch_sender(request.request_id).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(?e, "failed to create fetch sender");
                    return;
                }
            };
            deliver_fetch(&request, sender.as_ref()).await;
        });
    }

    async fn spawn_runner(
        runners: &mut JoinSet<()>,
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        cache_store: Arc<TrackCacheStore>,
        request: EgressStartRequest,
    ) {
        let publisher = session_repo
            .lock()
            .await
            .publisher(request.subscriber_session_id);
        let Some(publisher) = publisher else {
            tracing::error!("subscriber session not found for egress start");
            let _ = request
                .ready_sender
                .send(Err(anyhow::anyhow!("subscriber session not found")));
            return;
        };

        let cache = cache_store.get_or_create(&request.track_key);
        let track_alias = request.downstream_subscription.track_alias();
        let egress_track_span = tracing::info_span!(
            parent: &request.parent_span,
            "relay.dataplane.egress.track",
            subscriber_session_id = %request.subscriber_session_id,
            downstream_subscribe_id = request.downstream_subscribe_id,
            track_key = %request.track_key,
            track_alias = track_alias,
            track_namespace = %request.track_key.track_namespace,
            track_name = %request.track_key.track_name,
        );

        let runner = EgressRunner::new(
            request.track_key,
            cache,
            publisher,
            request.downstream_subscription.clone(),
            request.ready_sender,
            request.largest_location,
            request.forward_receiver,
        );

        let runner_stop_receiver = request.runner_stop_receiver;
        let subscribe_ok_receiver = request.subscribe_ok_receiver;
        runners.spawn(
            async move {
                if let Err(e) = runner
                    .run(runner_stop_receiver, subscribe_ok_receiver)
                    .await
                {
                    tracing::error!(?e, "egress runner finished with error");
                }
            }
            .instrument(egress_track_span),
        );
    }
}

impl Drop for EgressCoordinator {
    fn drop(&mut self) {
        self.command_runner.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::modules::data_plane::tests::harness::{
        MockPublisherObservers, fixtures::subscription::make_subscription,
        session_repository_with_downstream_session,
    };

    const SUBSCRIBER_SESSION_ID: SessionId = 2;
    const DOWNSTREAM_SUBSCRIBE_ID: u64 = 7;
    const TEST_TIMEOUT: Duration = Duration::from_secs(3);
    const PUBLISH_DONE_WINDOW: Duration = Duration::from_millis(100);

    struct TestContext {
        coordinator: EgressCoordinator,
        track_key: TrackKey,
        cache: Arc<TrackCache>,
        idle_cache_reference_count: usize,
        observers: MockPublisherObservers,
    }

    async fn setup() -> TestContext {
        let (session_repo, observers) =
            session_repository_with_downstream_session(SUBSCRIBER_SESSION_ID).await;
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let idle_cache_reference_count = Arc::strong_count(&cache);
        let coordinator = EgressCoordinator::new(session_repo, cache_store);
        TestContext {
            coordinator,
            track_key,
            cache,
            idle_cache_reference_count,
            observers,
        }
    }

    async fn start_reader_without_subscribe_ok(
        ctx: &TestContext,
        runner_stop_receiver: oneshot::Receiver<PublishDoneReason>,
    ) -> (
        Result<anyhow::Result<()>, oneshot::error::RecvError>,
        oneshot::Sender<()>,
    ) {
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (subscribe_ok_sender, subscribe_ok_receiver) = oneshot::channel();
        ctx.coordinator
            .sender()
            .send(EgressCommand::StartReader(Box::new(EgressStartRequest {
                subscriber_session_id: SUBSCRIBER_SESSION_ID,
                downstream_subscribe_id: DOWNSTREAM_SUBSCRIBE_ID,
                track_key: ctx.track_key.clone(),
                downstream_subscription: make_subscription(moqt::FilterType::LargestObject),
                parent_span: Span::none(),
                ready_sender,
                runner_stop_receiver,
                subscribe_ok_receiver,
                forward_receiver: watch::channel(true).1,
                largest_location: None,
            })))
            .await
            .expect("coordinator should accept commands");
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
            start_reader_without_subscribe_ok(ctx, runner_stop_receiver).await;
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
        let ctx = setup().await;
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
        let mut ctx = setup().await;
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
        let mut ctx = setup().await;
        let (runner_stop_sender, runner_stop_receiver) = oneshot::channel();
        let (readiness, subscribe_ok_sender) =
            start_reader_without_subscribe_ok(&ctx, runner_stop_receiver).await;
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
        let mut ctx = setup().await;
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
}

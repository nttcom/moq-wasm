use std::sync::Arc;

use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use tracing::{Instrument, Span};

use crate::logging::{link_span, relay_hostname};
use crate::modules::{
    data_plane::{
        cache::{cached_object::CacheStamp, track_cache::TrackCache},
        ingress::track_reader::read_track,
    },
    domain::{session_id::SessionId, track_key::TrackKey},
    session::{
        session_event::SessionEvent, subscriber::Subscriber, subscription::UpstreamSubscription,
    },
};

/// What every reader of one publisher's track shares.
#[derive(Clone)]
pub(crate) struct TrackIngest {
    pub(crate) track_key: TrackKey,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) publisher_mailbox_sender: mpsc::UnboundedSender<SessionEvent>,
    pub(crate) stop_receiver: watch::Receiver<bool>,
    pub(crate) stamp: CacheStamp,
}

pub(crate) struct IngestStart {
    pub(crate) track_key: TrackKey,
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) publisher_session_id: SessionId,
    pub(crate) subscriber: Box<dyn Subscriber>,
    pub(crate) subscription: UpstreamSubscription,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) publisher_mailbox_sender: mpsc::UnboundedSender<SessionEvent>,
    pub(crate) stamp: CacheStamp,
    pub(crate) publisher_session_span: Span,
}

/// The ingest of one publisher's track: waits for the publisher's data
/// receiver, then reads every subgroup stream or datagram into the track's
/// cache. Dropping it stops the readers.
pub(crate) struct IngestTask {
    publisher_mailbox_sender: mpsc::UnboundedSender<SessionEvent>,
    _stop_sender: watch::Sender<bool>,
    _join_handle: JoinHandle<()>,
}

impl IngestTask {
    /// Only links `request_span`: a clone kept by the task would hold the
    /// request span open until the track ends.
    pub(crate) fn run(start: IngestStart, request_span: &Span) -> Self {
        let (stop_sender, stop_receiver) = watch::channel(false);
        // The ingest outlives the request that starts it, so its span roots
        // its own trace, linked to the request and to the publisher session.
        let span = tracing::info_span!(
            parent: None,
            "relay.upstream.ingress",
            subscriber_session_id = start.subscriber_session_id,
            publisher_session_id = start.publisher_session_id,
            track_alias = start.subscription.track_alias(),
            track_namespace = %start.track_key.track_namespace,
            track_name = %start.track_key.track_name,
        );
        link_span(&span, request_span);
        link_span(&span, &start.publisher_session_span);
        let publisher_mailbox_sender = start.publisher_mailbox_sender.clone();
        let join_handle = tokio::spawn(Self::ingest(start, stop_receiver).instrument(span));
        Self {
            publisher_mailbox_sender,
            _stop_sender: stop_sender,
            _join_handle: join_handle,
        }
    }

    pub(crate) fn notify_publisher(&self, event: SessionEvent) {
        let _ = self.publisher_mailbox_sender.send(event);
    }

    async fn ingest(start: IngestStart, mut stop_receiver: watch::Receiver<bool>) {
        let IngestStart {
            track_key,
            subscriber_session_id,
            publisher_session_id,
            mut subscriber,
            subscription,
            cache,
            publisher_mailbox_sender,
            stamp,
            publisher_session_span,
            ..
        } = start;
        let _serving = ServingPublisher::start(
            &cache,
            publisher_session_id,
            subscription.as_moqt().track_properties().clone(),
        );
        tracing::debug!(track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, "creating upstream data receiver");
        let receiver = tokio::select! {
            _ = stop_receiver.changed() => {
                tracing::debug!(track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, "upstream ingress stopped before it started");
                return;
            }
            receiver = subscriber.create_data_receiver(&subscription) => receiver,
        };
        let receiver = match receiver {
            Ok(receiver) => receiver,
            Err(error) => {
                tracing::warn!(%error, track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, publisher_session_id, "failed to start upstream ingress");
                return;
            }
        };
        tracing::info!(track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, publisher_session_id, "track ingress started");
        let ingest = TrackIngest {
            track_key: track_key.clone(),
            cache: cache.clone(),
            publisher_mailbox_sender,
            stop_receiver,
            stamp,
        };
        let track_span = tracing::info_span!(
            parent: &publisher_session_span,
            "relay.dataplane.ingress.track",
            subscriber_session_id,
            publisher_session_id,
            track_alias = subscription.track_alias(),
            track_namespace = %track_key.track_namespace,
            track_name = %track_key.track_name,
        );
        link_span(&track_span, &Span::current());
        read_track(ingest, receiver, track_span.clone())
            .instrument(track_span)
            .await;
        tracing::info!(track_namespace = %track_key.track_namespace, track_name = %track_key.track_name, publisher_session_id, relay_hostname = relay_hostname(), "track ingress stopped");
    }
}

/// The publisher's track properties count in the cache's served set while
/// its ingest runs (see `TrackCache::publisher_started`).
struct ServingPublisher<'a> {
    cache: &'a TrackCache,
    publisher_session_id: SessionId,
}

impl<'a> ServingPublisher<'a> {
    fn start(
        cache: &'a TrackCache,
        publisher_session_id: SessionId,
        track_properties: moqt::TrackProperties,
    ) -> Self {
        cache.publisher_started(publisher_session_id, track_properties);
        Self {
            cache,
            publisher_session_id,
        }
    }
}

impl Drop for ServingPublisher<'_> {
    fn drop(&mut self) {
        self.cache.publisher_stopped(self.publisher_session_id);
    }
}

#[cfg(test)]
impl IngestTask {
    /// An ingest that reads nothing, for producers under test.
    pub(crate) fn idle() -> Self {
        Self::idle_notifying(mpsc::unbounded_channel().0)
    }

    pub(crate) fn idle_notifying(
        publisher_mailbox_sender: mpsc::UnboundedSender<SessionEvent>,
    ) -> Self {
        let (stop_sender, _stop_receiver) = watch::channel(false);
        Self {
            publisher_mailbox_sender,
            _stop_sender: stop_sender,
            _join_handle: tokio::spawn(async {}),
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use crate::modules::{
        session::{Session, data_receiver::stream_receiver::StreamReceiver},
        test_support::{
            closed_spans::ClosedSpans,
            mock_session::{MockUpstreamSession, RecordedControlMessages},
            relay_harness::{
                UpstreamSubgroupStream,
                fixtures::{location, subscription::upstream_subscription},
                wait_largest_location,
            },
        },
    };

    const FIRST_PUBLISHER: SessionId = 1;
    const SECOND_PUBLISHER: SessionId = 2;

    struct UpstreamPublisher {
        recorded: RecordedControlMessages,
        stream_sender: mpsc::UnboundedSender<Box<dyn StreamReceiver>>,
        _ingest: IngestTask,
    }

    impl UpstreamPublisher {
        fn open_stream(&self) -> UpstreamSubgroupStream {
            UpstreamSubgroupStream::open(|receiver| {
                self.stream_sender
                    .send(receiver)
                    .expect("ingest should accept streams");
                tokio::spawn(async {})
            })
        }
    }

    /// Starts the ingest of `publisher_session_id`, whose upstream request id
    /// equals its session id, and waits until it created the publisher's
    /// data receiver.
    async fn start(cache: &Arc<TrackCache>, publisher_session_id: SessionId) -> UpstreamPublisher {
        start_in_request(cache, publisher_session_id, &Span::none()).await
    }

    async fn start_in_request(
        cache: &Arc<TrackCache>,
        publisher_session_id: SessionId,
        request_span: &Span,
    ) -> UpstreamPublisher {
        let recorded = RecordedControlMessages::default();
        let stream_sender = recorded.feed_streams();
        let session = MockUpstreamSession::new(recorded.clone());
        let (publisher_mailbox_sender, _publisher_mailbox_receiver) = mpsc::unbounded_channel();
        let ingest = IngestTask::run(
            IngestStart {
                track_key: TrackKey::new("ns", "track"),
                subscriber_session_id: 3,
                publisher_session_id,
                subscriber: session.as_subscriber(),
                subscription: upstream_subscription(publisher_session_id),
                cache: cache.clone(),
                publisher_mailbox_sender,
                stamp: CacheStamp::default(),
                publisher_session_span: Span::none(),
            },
            request_span,
        );
        recorded.wait_ingest_started(publisher_session_id).await;
        UpstreamPublisher {
            recorded,
            stream_sender,
            _ingest: ingest,
        }
    }

    #[tokio::test]
    async fn the_request_span_closes_while_the_ingest_runs() {
        // Arrange
        let closed_spans = ClosedSpans::default();
        let _default = telemetry::testing::set_default(
            tracing_subscriber::registry().with(closed_spans.clone()),
        );
        let cache = Arc::new(TrackCache::new());
        let request_span = tracing::info_span!("relay.publish");

        // Act
        let _publisher = start_in_request(&cache, FIRST_PUBLISHER, &request_span).await;
        drop(request_span);

        // Assert
        assert!(
            closed_spans.contains("relay.publish"),
            "closed spans: {:?}",
            closed_spans.names()
        );
    }

    #[tokio::test]
    async fn a_second_publisher_of_an_active_track_is_ingested() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let _first = start(&cache, FIRST_PUBLISHER).await;
        let second = start(&cache, SECOND_PUBLISHER).await;

        // Act
        let stream = second.open_stream();
        stream.header(0);
        stream.object(0);

        // Assert
        wait_largest_location(&cache, location(0, 0)).await;
    }

    #[tokio::test]
    async fn dropping_one_ingest_stops_its_reader_and_keeps_the_other() {
        // Arrange
        let cache = Arc::new(TrackCache::new());
        let first = start(&cache, FIRST_PUBLISHER).await;
        let second = start(&cache, SECOND_PUBLISHER).await;
        let first_stream = first.open_stream();
        first_stream.header(0);
        first_stream.object(0);
        wait_largest_location(&cache, location(0, 0)).await;
        let first_recorded = first.recorded.clone();

        // Act
        drop(first);
        let second_stream = second.open_stream();
        second_stream.header(1);
        second_stream.object(0);

        // Assert
        first_recorded.wait_ingest_stopped(FIRST_PUBLISHER).await;
        wait_largest_location(&cache, location(1, 0)).await;
    }
}

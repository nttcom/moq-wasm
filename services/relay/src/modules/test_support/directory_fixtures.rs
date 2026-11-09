use std::sync::Arc;

use moqt::{CertificateVerification, namespace::NamespaceInterest, object::ContentExists};

use crate::modules::{
    auth::verified_token::VerifiedToken,
    cascading::{
        inter_relay_connection_manager::{InterRelayClientConfig, InterRelayConnectionManager},
        route_registry::NoopRelayRouteRegistry,
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{IngestRequest, downstream_publish::DownstreamPublish, start_ingest},
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::{cache::store::TrackCacheStore, ingress::ingest_task::IngestTask},
    directory::{
        InMemoryLocalPubSubDirectory,
        track::{
            DownstreamSubscription, NamespaceSubscription, Producer, ProducerOrigin, ProducerState,
            TrackState,
        },
    },
    domain::{
        delivery_control::DeliveryControl, session_id::SessionId, session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::session_repository::NewSession,
    test_support::{
        mock_session::{
            MockUpstreamSession, RecordedControlMessages, detached_session_repository,
            mock_session_answering_publish, session_repository_with_session,
            session_repository_with_upstream_session, wait_until,
        },
        relay_harness::fixtures::subscription::{
            upstream_subscription, upstream_subscription_with_track_properties,
        },
    },
};

pub(crate) const PUBLISHER_SESSION: SessionId = 1;
pub(crate) const NAMESPACE_SUBSCRIBER_SESSION: SessionId = 2;
pub(crate) const UPSTREAM_REQUEST_ID: u64 = 42;

/// A namespace subscription as draft-14 makes it: namespaces and tracks,
/// Forward State 1, the subscriber's own tracks echoed back.
pub(crate) fn namespaces_and_tracks(peer: SessionPeer) -> NamespaceSubscription {
    NamespaceSubscription {
        interest: NamespaceInterest::NamespacesAndTracks,
        peer,
        forward: true,
    }
}

/// A draft-18 SUBSCRIBE_NAMESPACE (`Namespaces`) or SUBSCRIBE_TRACKS
/// (`Tracks`, Forward State 1) of a client.
pub(crate) fn client_subscription(interest: NamespaceInterest) -> NamespaceSubscription {
    NamespaceSubscription {
        interest,
        peer: SessionPeer::Client,
        forward: true,
    }
}

pub(crate) fn track_key() -> TrackKey {
    TrackKey::new("ns", "track")
}

/// An open-ended subscription starting at {0, 0}.
pub(crate) fn delivery_control(forward: bool) -> DeliveryControl {
    DeliveryControl::new(forward, &moqt::subscribe::FilterType::LargestObject, None)
}

/// A Largest Object subscription to `track_key` accepted at `joining_location`.
pub(crate) fn downstream_subscription(
    track_key: TrackKey,
    joining_location: Option<moqt::object::Location>,
) -> DownstreamSubscription {
    DownstreamSubscription {
        track_key,
        filter_type: moqt::subscribe::FilterType::LargestObject,
        joining_location,
    }
}

pub(crate) fn active_upstream(origin: ProducerOrigin) -> ProducerState {
    ProducerState {
        subscription: upstream_subscription(UPSTREAM_REQUEST_ID),
        expires: None,
        content_exists: ContentExists::False,
        origin,
        peer: SessionPeer::Client,
    }
}

/// A SUBSCRIBE_OK'd upstream subscription whose answer said
/// `track_properties`.
pub(crate) fn upstream_carrying(track_properties: moqt::TrackProperties) -> ProducerState {
    ProducerState {
        subscription: upstream_subscription_with_track_properties(
            UPSTREAM_REQUEST_ID,
            track_properties,
        ),
        ..active_upstream(ProducerOrigin::Subscribe)
    }
}

pub(crate) fn producer_with(state: ProducerState) -> Producer {
    Producer::new(state, IngestTask::idle())
}

pub(crate) fn producer(origin: ProducerOrigin) -> Producer {
    producer_with(active_upstream(origin))
}

pub(crate) fn upstream_track(origin: ProducerOrigin) -> TrackState {
    let mut upstream_track = TrackState::default();
    upstream_track
        .producers
        .insert(PUBLISHER_SESSION, active_upstream(origin));
    upstream_track
}

pub(crate) fn table_with_upstream(
    origin: ProducerOrigin,
) -> (InMemoryLocalPubSubDirectory, TrackKey) {
    let table = InMemoryLocalPubSubDirectory::new();
    table.register_upstream_subscription(track_key(), PUBLISHER_SESSION, producer(origin));
    (table, track_key())
}

pub(crate) struct UpstreamReleaseContext {
    pub(crate) table: InMemoryLocalPubSubDirectory,
    pub(crate) forwarder: ControlMessageForwarder,
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) recorded: RecordedControlMessages,
}

/// `PUBLISHER_SESSION` feeds `ns/track` through an ingest reading the mock
/// session, so `recorded` sees the ingest start and stop.
pub(crate) async fn upstream_release_context(origin: ProducerOrigin) -> UpstreamReleaseContext {
    let (repository, recorded) = session_repository_with_upstream_session(PUBLISHER_SESSION).await;
    let ctx = UpstreamReleaseContext {
        table: InMemoryLocalPubSubDirectory::new(),
        forwarder: ControlMessageForwarder { repository },
        cache_store: Arc::new(TrackCacheStore::new()),
        recorded,
    };
    ctx.feed(PUBLISHER_SESSION, active_upstream(origin), &ctx.recorded)
        .await;
    ctx
}

impl UpstreamReleaseContext {
    /// Adds a mock publisher session feeding the track through its own ingest.
    pub(crate) async fn add_publisher(
        &self,
        session_id: SessionId,
        upstream_request_id: u64,
    ) -> RecordedControlMessages {
        let recorded = RecordedControlMessages::default();
        self.forwarder.repository.lock().await.add(NewSession {
            session_id,
            session: Box::new(MockUpstreamSession::new(recorded.clone())),
            session_span: tracing::Span::none(),
            peer: SessionPeer::Client,
            verified_token: VerifiedToken::full_access(),
        });
        let state = ProducerState {
            subscription: upstream_subscription(upstream_request_id),
            ..active_upstream(ProducerOrigin::Subscribe)
        };
        self.feed(session_id, state, &recorded).await;
        recorded
    }

    async fn feed(
        &self,
        session_id: SessionId,
        state: ProducerState,
        recorded: &RecordedControlMessages,
    ) {
        let upstream_request_id = state.subscription.request_id();
        let ingest = start_ingest(
            &self.forwarder,
            &self.cache_store,
            IngestRequest {
                subscriber_session_id: 2,
                publisher_session_id: session_id,
                track_key: track_key(),
                subscription: state.subscription.clone(),
            },
        )
        .await
        .expect("the publisher session is in the repository");
        self.table.register_upstream_subscription(
            track_key(),
            session_id,
            Producer::new(state, ingest),
        );
        recorded.wait_ingest_started(upstream_request_id).await;
    }
}

pub(crate) fn unused_inter_relay_client_config() -> InterRelayClientConfig {
    InterRelayClientConfig {
        relay_token: "unused-relay-token".to_string(),
        certificate_verification: CertificateVerification::NativeRoots,
        versions: moqt::DEFAULT_VERSIONS.to_vec(),
    }
}

pub(crate) fn local_publisher_resolver() -> UpstreamPublisherResolver {
    UpstreamPublisherResolver::new(
        Arc::new(NoopRelayRouteRegistry),
        Arc::new(InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(detached_session_repository())),
            unused_inter_relay_client_config(),
        )),
    )
}

pub(crate) struct DownstreamPublishContext {
    pub(crate) table: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) forwarder: ControlMessageForwarder,
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) subscriber: RecordedControlMessages,
}

impl DownstreamPublishContext {
    pub(crate) fn downstream_publish(&self) -> DownstreamPublish {
        DownstreamPublish {
            table: self.table.clone(),
            forwarder: self.forwarder.clone(),
            cache_store: self.cache_store.clone(),
        }
    }

    /// Resolves once the forwarded PUBLISH `request_id` is registered for the
    /// namespace subscriber.
    pub(crate) async fn wait_registered(&self, request_id: u64) {
        wait_until(|| {
            self.table
                .get_downstream_subscription(NAMESPACE_SUBSCRIBER_SESSION, request_id)
                .is_some()
        })
        .await
        .expect("the forwarded PUBLISH should be registered");
    }
}

pub(crate) async fn downstream_publish_context(
    table: Arc<InMemoryLocalPubSubDirectory>,
    answer_publish: impl Fn() -> anyhow::Result<bool> + Send + Sync + 'static,
) -> DownstreamPublishContext {
    let (session, subscriber) = mock_session_answering_publish(answer_publish);
    let repository = session_repository_with_session(
        NAMESPACE_SUBSCRIBER_SESSION,
        session,
        VerifiedToken::full_access(),
    )
    .await;
    DownstreamPublishContext {
        table,
        forwarder: ControlMessageForwarder { repository },
        cache_store: Arc::new(TrackCacheStore::new()),
        subscriber,
    }
}

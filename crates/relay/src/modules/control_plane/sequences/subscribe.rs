use moqt::RequestErrorCode;
use std::sync::Arc;

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::cache::store::TrackCacheStore,
    directory::{
        InMemoryLocalPubSubDirectory, TrackLookup, TrackReservation,
        track::{CreationFailure, CreationState, TrackState},
    },
    domain::{
        delivery_control::DeliveryControl, session_id::SessionId, session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::handler::subscribe::SubscribeHandler,
};
use moqt::fetch::JoinPoint;

use moqt::object::ContentExists;

pub(crate) mod pending_producer_task;

use pending_producer_task::{PendingProducerDeps, subscribe_publishers};

pub(crate) struct Subscribe;

/// Another session's cleanup can remove the track between finding it and
/// registering on it; each attempt finds or creates it again.
const UPSTREAM_ATTEMPTS: usize = 3;

/// Clients that leave Forward at 0 in SUBSCRIBE still expect delivery.
const INITIAL_FORWARD: bool = true;

enum Acceptance {
    Answered,
    UpstreamGone,
}

/// The creation's outcome once it is no longer `Creating`; `None` when the
/// track entry went away first.
async fn wait_created(
    mut receiver: tokio::sync::watch::Receiver<CreationState>,
) -> Option<CreationState> {
    receiver
        .wait_for(|state| *state != CreationState::Creating)
        .await
        .ok()
        .map(|state| state.clone())
}

pub(super) fn cached_largest(
    cache_store: &TrackCacheStore,
    track_key: &TrackKey,
) -> Option<moqt::object::Location> {
    cache_store
        .get(track_key)
        .and_then(|cache| cache.largest_location())
}

/// The upstream SUBSCRIBE uses the Largest Object filter, which starts after
/// the Largest Location its SUBSCRIBE_OK reports (§9.7), so the cache lacks
/// that object until the track's next one; a meeting catalog sends that only
/// when it changes.
pub(super) fn existing_track_largest(
    cache_store: &TrackCacheStore,
    track_key: &TrackKey,
    upstream_track: &TrackState,
) -> Option<moqt::object::Location> {
    let upstream_largest = match upstream_track.content_exists() {
        ContentExists::True { location } => Some(location),
        ContentExists::False => None,
    };
    cached_largest(cache_store, track_key).max(upstream_largest)
}

/// The track as found or created for a SUBSCRIBE, held wanting its producers
/// (`_reservation`) until the downstream subscription is registered.
struct UpstreamTrackAccess {
    track_key: TrackKey,
    upstream_track: TrackState,
    largest_location: Option<moqt::object::Location>,
    _reservation: TrackReservation,
}

pub(crate) enum UpstreamSubscriptionError {
    PublisherNotFound,
    SubscribeFailed(anyhow::Error),
}

impl UpstreamSubscriptionError {
    pub(crate) fn failure(&self) -> CreationFailure {
        let (code, reason) = self.subscribe_error_response();
        CreationFailure { code, reason }
    }

    /// SUBSCRIBE_ERROR (code, reason) for the downstream subscriber. An
    /// upstream SUBSCRIBE_ERROR is relayed verbatim; an upstream timeout maps
    /// to TIMEOUT so one slow upstream request stays a request-scoped failure.
    fn subscribe_error_response(&self) -> (RequestErrorCode, String) {
        match self {
            Self::PublisherNotFound => (
                RequestErrorCode::TrackDoesNotExist,
                "Designated namespace and track name do not exist.".to_string(),
            ),
            Self::SubscribeFailed(error) => match error.downcast_ref::<moqt::RequestFailure>() {
                Some(moqt::RequestFailure::Rejected { code, reason, .. }) => {
                    (*code, reason.clone())
                }
                Some(moqt::RequestFailure::Timeout) => (
                    RequestErrorCode::Timeout,
                    "Upstream subscribe timed out".to_string(),
                ),
                _ => (
                    RequestErrorCode::InternalError,
                    "Failed to create upstream subscription.".to_string(),
                ),
            },
        }
    }
}

impl Subscribe {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        cache_store: &Arc<TrackCacheStore>,
        handler: Box<dyn SubscribeHandler>,
    ) {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::debug!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            "SUBSCRIBE received"
        );
        let requester = super::session_peer(session_id, forwarder).await;

        for _ in 0..UPSTREAM_ATTEMPTS {
            let access = match self
                .get_or_create_upstream_subscription(
                    session_id,
                    requester,
                    track_namespace,
                    track_name,
                    table,
                    forwarder,
                    upstream_publisher_resolver,
                    cache_store,
                )
                .await
            {
                Ok(upstream_subscription) => upstream_subscription,
                Err(failure) => {
                    if let Err(send_error) = self
                        .response_error(session_id, handler.as_ref(), failure.code, failure.reason)
                        .await
                    {
                        tracing::error!(
                            request_id = handler.subscribe_id(),
                            track_namespace = %track_namespace,
                            track_name = %track_name,
                            error = ?send_error,
                            "failed to send SUBSCRIBE_ERROR"
                        );
                    }
                    return;
                }
            };

            if let Some((publisher_session_id, upstream)) = access
                .upstream_track
                .producers
                .iter()
                .rev()
                .find(|(_, producer)| producer.peer == SessionPeer::Relay)
            {
                super::record_upstream_relay(
                    forwarder,
                    *publisher_session_id,
                    upstream.subscription.request_id(),
                )
                .await;
            }
            let acceptance = self
                .accept_downstream_subscription(
                    session_id,
                    requester,
                    access.track_key,
                    access.upstream_track,
                    access.largest_location,
                    table,
                    forwarder,
                    cache_store,
                    handler.as_ref(),
                )
                .await;
            if let Acceptance::Answered = acceptance {
                return;
            }
            tracing::info!(
                request_id = handler.subscribe_id(),
                track_namespace = %track_namespace,
                track_name = %track_name,
                "upstream subscription ended before the downstream subscription was accepted; retrying"
            );
        }
        self.response_track_gone(session_id, handler.as_ref()).await;
    }

    /// Finds the track or creates it: the first SUBSCRIBE for a track sends
    /// the upstream SUBSCRIBEs and every SUBSCRIBE for it waits for the first
    /// producer, or for the creation to fail (draft-14 §8.4).
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.subscribe.get_or_create_upstream_subscription",
        skip_all,
        fields(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name
        )
    )]
    async fn get_or_create_upstream_subscription(
        &self,
        session_id: SessionId,
        requester: SessionPeer,
        track_namespace: &str,
        track_name: &str,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        cache_store: &Arc<TrackCacheStore>,
    ) -> Result<UpstreamTrackAccess, CreationFailure> {
        let track_key = TrackKey::new(track_namespace, track_name);
        loop {
            let (receiver, reservation, cache_before_subscribe) =
                match table.track_or_begin_creation(&track_key, requester) {
                    TrackLookup::Ready(upstream_track, reservation) => {
                        let largest_location =
                            existing_track_largest(cache_store, &track_key, &upstream_track);
                        return Ok(UpstreamTrackAccess {
                            track_key,
                            upstream_track,
                            largest_location,
                            _reservation: reservation,
                        });
                    }
                    TrackLookup::Creating(receiver, reservation) => {
                        tracing::debug!("another SUBSCRIBE is creating the track; waiting");
                        (receiver, reservation, None)
                    }
                    TrackLookup::Started(receiver, reservation) => {
                        let cache_before_subscribe = cached_largest(cache_store, &track_key);
                        self.create_upstream_subscription(
                            session_id,
                            requester,
                            &track_key,
                            table,
                            forwarder,
                            upstream_publisher_resolver,
                            cache_store,
                        )
                        .await;
                        (receiver, reservation, Some(cache_before_subscribe))
                    }
                };
            if let Some(CreationState::Failed(failure)) = wait_created(receiver).await {
                return Err(failure);
            }
            // Created, or the entry went away meanwhile: then look again.
            let Some(upstream_track) = table.track_state(&track_key) else {
                continue;
            };
            // The creator advertises the Largest Location of subscribe time:
            // the upstream SUBSCRIBE uses the Largest Object filter, so the
            // objects a burst cached meanwhile are delivered, not skipped.
            let largest_location = match cache_before_subscribe {
                Some(cache_before_subscribe) => {
                    let upstream_largest = match upstream_track.content_exists() {
                        ContentExists::True { location } => Some(location),
                        ContentExists::False => None,
                    };
                    upstream_largest.max(cache_before_subscribe)
                }
                None => existing_track_largest(cache_store, &track_key, &upstream_track),
            };
            return Ok(UpstreamTrackAccess {
                track_key,
                upstream_track,
                largest_location,
                _reservation: reservation,
            });
        }
    }

    /// Resolves the track's publishers and sends each a SUBSCRIBE in a
    /// `PendingProducerTask`; without any, the creation is abandoned, which
    /// fails it unless a PUBLISH created the track meanwhile.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.subscribe.create_upstream_subscription",
        skip_all,
        fields(
            session_id = %session_id,
            track_namespace = %track_key.track_namespace,
            track_name = %track_key.track_name
        )
    )]
    async fn create_upstream_subscription(
        &self,
        session_id: SessionId,
        requester: SessionPeer,
        track_key: &TrackKey,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        cache_store: &Arc<TrackCacheStore>,
    ) {
        if requester == SessionPeer::Client {
            upstream_publisher_resolver
                .watched_namespace_routes
                .watch(&track_key.track_namespace)
                .await;
        }
        let publishers = upstream_publisher_resolver
            .resolve(
                table,
                &track_key.track_namespace,
                &track_key.track_name,
                requester,
            )
            .await;
        if publishers.is_empty() {
            table.pending_producer_ended(
                track_key,
                Some(UpstreamSubscriptionError::PublisherNotFound.failure()),
            );
            return;
        }
        subscribe_publishers(
            &PendingProducerDeps {
                table: table.clone(),
                forwarder: forwarder.clone(),
                cache_store: cache_store.clone(),
                resolver: upstream_publisher_resolver.clone(),
            },
            track_key,
            session_id,
            publishers,
        );
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.subscribe.accept_downstream_subscription",
        skip_all,
        fields(
            session_id = %session_id
        )
    )]
    async fn accept_downstream_subscription(
        &self,
        session_id: SessionId,
        subscriber_peer: SessionPeer,
        track_key: TrackKey,
        upstream_track: TrackState,
        largest_location: Option<moqt::object::Location>,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        cache_store: &Arc<TrackCacheStore>,
        handler: &dyn SubscribeHandler,
    ) -> Acceptance {
        if cache_store
            .get(&track_key)
            .is_some_and(|cache| cache.is_malformed())
        {
            let _ = self
                .response_error(
                    session_id,
                    handler,
                    RequestErrorCode::TrackDoesNotExist,
                    "malformed track".to_string(),
                )
                .await;
            return Acceptance::Answered;
        }

        let subscriber_track_alias = handler.allocate_track_alias();

        let content_exists = match largest_location {
            Some(location) => ContentExists::True { location },
            None => upstream_track.content_exists(),
        };

        let downstream_subscription = handler.to_downstream_subscription(subscriber_track_alias);
        let filter_type = downstream_subscription.filter_type();
        let Some(runner_signals) = table.register_downstream_subscription(
            session_id,
            handler.subscribe_id(),
            subscriber_peer,
            track_key.clone(),
            JoinPoint::new(&filter_type, largest_location),
            DeliveryControl::new(INITIAL_FORWARD, &filter_type, largest_location),
        ) else {
            tracing::debug!(
                request_id = handler.subscribe_id(),
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                "upstream subscription removed before the downstream subscription was registered"
            );
            return Acceptance::UpstreamGone;
        };

        let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
        let (subscribe_ok_sender, subscribe_ok_receiver) = tokio::sync::oneshot::channel();
        if !super::start_egress(
            forwarder,
            cache_store,
            super::EgressRequest {
                subscriber_session_id: session_id,
                request_id: handler.subscribe_id(),
                track_key: track_key.clone(),
                downstream_subscription,
                ready_sender,
                runner_signals,
                subscribe_ok_receiver,
                largest_location,
            },
        )
        .await
        {
            return Acceptance::Answered;
        }
        match ready_receiver.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(
                    ?error,
                    request_id = handler.subscribe_id(),
                    "failed to start egress runner"
                );
                return Acceptance::Answered;
            }
            Err(_) => {
                tracing::debug!(
                    request_id = handler.subscribe_id(),
                    "downstream subscription removed before its egress runner became ready"
                );
                return Acceptance::UpstreamGone;
            }
        }

        if handler
            .ok_with_track_alias(
                subscriber_track_alias,
                upstream_track.expires().unwrap_or(0),
                content_exists,
                upstream_track.max_cache_duration(),
            )
            .await
            .is_err()
        {
            tracing::error!(
                request_id = handler.subscribe_id(),
                track_alias = subscriber_track_alias,
                "failed to send SUBSCRIBE_OK"
            );
            return Acceptance::Answered;
        }
        let _ = subscribe_ok_sender.send(());
        tracing::info!(
            session_id = %session_id,
            request_id = handler.subscribe_id(),
            track_namespace = %track_key.track_namespace,
            track_name = %track_key.track_name,
            track_alias = subscriber_track_alias,
            "SUBSCRIBE accepted"
        );
        Acceptance::Answered
    }

    async fn response_track_gone(&self, session_id: SessionId, handler: &dyn SubscribeHandler) {
        let _ = self
            .response_error(
                session_id,
                handler,
                RequestErrorCode::TrackDoesNotExist,
                "track is no longer available".to_string(),
            )
            .await;
    }

    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.subscribe.response_error",
        skip_all
    )]
    async fn response_error(
        &self,
        session_id: SessionId,
        handler: &dyn SubscribeHandler,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::warn!(
            session_id,
            request_id = handler.subscribe_id(),
            track_namespace = %track_namespace,
            track_name = %track_name,
            error_code = ?code,
            reason_phrase = %reason_phrase,
            "Sending `SUBSCRIBE_ERROR`"
        );
        handler.error(code, reason_phrase).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;
    use crate::modules::auth::verified_token::VerifiedToken;
    use crate::modules::cascading::inter_relay_connection_manager::InterRelayConnectionManager;
    use crate::modules::cascading::route_registry::NoopRelayRouteRegistry;
    use crate::modules::data_plane::cache::track_cache::TrackCache;
    use crate::modules::session::session_repository::SessionRepository;
    use crate::modules::test_support::directory_fixtures::delivery_control;
    use crate::modules::test_support::directory_fixtures::{
        PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, local_publisher_resolver, producer, track_key,
        upstream_track,
    };
    use crate::modules::test_support::mock_session::{
        MockSubscribeHandler, MockUpstreamSession, RecordedControlMessages,
        detached_session_repository, mock_session_answering_subscribe,
        mock_session_never_answering_subscribe, recorded_session_answering_subscribe,
        session_repository_with_session, session_repository_with_sessions,
    };
    use crate::modules::test_support::relay_harness::fixtures::cached_object::insert_closed_group;
    use crate::modules::{
        directory::{InMemoryLocalPubSubDirectory, track::ProducerOrigin},
        domain::session_peer::SessionPeer,
    };

    fn append_one_object(cache: &TrackCache, group_id: u64) {
        insert_closed_group(cache, group_id, &[0]);
    }

    #[test]
    fn upstream_timeout_maps_to_subscribe_error_timeout() {
        let error =
            UpstreamSubscriptionError::SubscribeFailed(moqt::RequestFailure::Timeout.into());
        let (code, _reason) = error.subscribe_error_response();
        assert_eq!(code, RequestErrorCode::Timeout);
    }

    #[test]
    fn upstream_subscribe_error_code_is_relayed_verbatim() {
        let error = UpstreamSubscriptionError::SubscribeFailed(anyhow::Error::new(
            moqt::RequestFailure::Rejected {
                request_id: 7,
                code: RequestErrorCode::Unauthorized,
                reason: "unauthorized".to_string(),
            },
        ));
        let (code, reason) = error.subscribe_error_response();
        assert_eq!(code, RequestErrorCode::Unauthorized);
        assert_eq!(reason, "unauthorized");
    }

    #[test]
    fn publisher_not_found_maps_to_track_does_not_exist() {
        let (code, _reason) =
            UpstreamSubscriptionError::PublisherNotFound.subscribe_error_response();
        assert_eq!(code, RequestErrorCode::TrackDoesNotExist);
    }

    async fn create_upstream_and_resolve_largest(
        cache_store: Arc<TrackCacheStore>,
        track_key: TrackKey,
        content_exists: moqt::object::ContentExists,
        bursts_on_subscribe: bool,
    ) -> Option<moqt::object::Location> {
        resolve_largest_per_subscribe(
            cache_store,
            track_key,
            content_exists,
            bursts_on_subscribe,
            1,
        )
        .await[0]
    }

    async fn resolve_largest_per_subscribe(
        cache_store: Arc<TrackCacheStore>,
        track_key: TrackKey,
        content_exists: moqt::object::ContentExists,
        bursts_on_subscribe: bool,
        subscribe_count: usize,
    ) -> Vec<Option<moqt::object::Location>> {
        const SUBSCRIBER_SESSION: SessionId = 2;

        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(
            PUBLISHER_SESSION,
            track_key.track_namespace.clone(),
            SessionPeer::Client,
        );

        let session = mock_session_answering_subscribe({
            let cache_store = cache_store.clone();
            let track_key = track_key.clone();
            move || {
                if bursts_on_subscribe {
                    append_one_object(&cache_store.get_or_create(&track_key), 0);
                }
                content_exists
            }
        });
        let repository = session_repository_with_session(
            PUBLISHER_SESSION,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let forwarder = ControlMessageForwarder { repository };
        let resolver = Arc::new(local_publisher_resolver());

        let mut largest_locations = Vec::new();
        for _ in 0..subscribe_count {
            let Ok(UpstreamTrackAccess {
                largest_location, ..
            }) = Subscribe
                .get_or_create_upstream_subscription(
                    SUBSCRIBER_SESSION,
                    SessionPeer::Client,
                    &track_key.track_namespace,
                    &track_key.track_name,
                    &table,
                    &forwarder,
                    &resolver,
                    &cache_store,
                )
                .await
            else {
                panic!("upstream subscription should be created");
            };
            largest_locations.push(largest_location);
        }
        largest_locations
    }

    const DEAD_PUBLISHER_SESSION: SessionId = 1;
    const NEW_PUBLISHER_SESSION: SessionId = 3;

    async fn subscribe_to_publishers(
        publishers: Vec<(SessionId, Box<dyn crate::modules::session::Session>)>,
    ) -> (
        Arc<InMemoryLocalPubSubDirectory>,
        Result<UpstreamTrackAccess, CreationFailure>,
    ) {
        let publisher_session_ids = publishers.iter().map(|(id, _)| *id).collect();
        let repository =
            session_repository_with_sessions(publishers, VerifiedToken::full_access()).await;
        subscribe_through(repository, publisher_session_ids).await
    }

    async fn subscribe_through(
        repository: Arc<tokio::sync::Mutex<SessionRepository>>,
        publisher_session_ids: Vec<SessionId>,
    ) -> (
        Arc<InMemoryLocalPubSubDirectory>,
        Result<UpstreamTrackAccess, CreationFailure>,
    ) {
        const SUBSCRIBER_SESSION: SessionId = 2;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        for publisher_session_id in publisher_session_ids {
            table.register_publish_namespace(
                publisher_session_id,
                "ns".to_string(),
                SessionPeer::Client,
            );
        }
        let forwarder = ControlMessageForwarder { repository };
        let resolver = Arc::new(local_publisher_resolver());
        let cache_store = Arc::new(TrackCacheStore::new());
        let created = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            Subscribe.get_or_create_upstream_subscription(
                SUBSCRIBER_SESSION,
                SessionPeer::Client,
                "ns",
                "track",
                &table,
                &forwarder,
                &resolver,
                &cache_store,
            ),
        )
        .await
        .expect("the SUBSCRIBE must not wait for a publisher that never answers");
        if let Ok(access) = &created {
            table
                .register_downstream_subscription(
                    SUBSCRIBER_SESSION,
                    1,
                    SessionPeer::Client,
                    access.track_key.clone(),
                    JoinPoint::Largest(None),
                    delivery_control(true),
                )
                .unwrap();
        }
        (table, created)
    }

    async fn wait_track_publishers(
        table: &InMemoryLocalPubSubDirectory,
        expected: Vec<SessionId>,
    ) -> Vec<SessionId> {
        let track_key = TrackKey::new("ns", "track");
        let publishers = || {
            table
                .track_state(&track_key)
                .map(|track| track.producers.into_keys().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while publishers() != expected {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await;
        publishers()
    }

    #[tokio::test]
    async fn a_publisher_that_never_answers_does_not_hold_back_the_subscribe() {
        // Arrange
        let publishers = vec![
            (
                DEAD_PUBLISHER_SESSION,
                mock_session_never_answering_subscribe(),
            ),
            (
                NEW_PUBLISHER_SESSION,
                mock_session_answering_subscribe(|| moqt::object::ContentExists::False),
            ),
        ];

        // Act
        let (table, created) = subscribe_to_publishers(publishers).await;

        // Assert
        assert!(created.is_ok());
        assert_eq!(
            wait_track_publishers(&table, vec![NEW_PUBLISHER_SESSION]).await,
            vec![NEW_PUBLISHER_SESSION]
        );
    }

    #[tokio::test]
    async fn every_answering_publisher_feeds_the_track() {
        // Arrange
        let publishers = vec![
            (
                DEAD_PUBLISHER_SESSION,
                mock_session_answering_subscribe(|| moqt::object::ContentExists::False),
            ),
            (
                NEW_PUBLISHER_SESSION,
                mock_session_answering_subscribe(|| moqt::object::ContentExists::False),
            ),
        ];

        // Act
        let (table, created) = subscribe_to_publishers(publishers).await;

        // Assert
        assert!(created.is_ok());
        assert_eq!(
            wait_track_publishers(&table, vec![DEAD_PUBLISHER_SESSION, NEW_PUBLISHER_SESSION])
                .await,
            vec![DEAD_PUBLISHER_SESSION, NEW_PUBLISHER_SESSION]
        );
    }

    #[tokio::test]
    async fn a_publish_overtaking_the_subscribe_keeps_feeding_the_track() {
        // Arrange: the publisher sends PUBLISH for the track as it answers the SUBSCRIBE
        let table_cell = Arc::new(OnceLock::<Arc<InMemoryLocalPubSubDirectory>>::new());
        let publishing_cell = table_cell.clone();
        let (session, recorded) = recorded_session_answering_subscribe(move || {
            publishing_cell
                .get()
                .unwrap()
                .register_upstream_subscription(
                    track_key(),
                    PUBLISHER_SESSION,
                    producer(ProducerOrigin::Publish),
                );
            moqt::object::ContentExists::False
        });
        let repository = session_repository_with_session(
            PUBLISHER_SESSION,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(PUBLISHER_SESSION, "ns".to_string(), SessionPeer::Client);
        let _ = table_cell.set(table.clone());
        let forwarder = ControlMessageForwarder { repository };

        // Act
        let created = Subscribe
            .get_or_create_upstream_subscription(
                2,
                SessionPeer::Client,
                "ns",
                "track",
                &table,
                &forwarder,
                &Arc::new(local_publisher_resolver()),
                &Arc::new(TrackCacheStore::new()),
            )
            .await;

        // Assert: only the SUBSCRIBE (request 1) is unsubscribed, and its ingest never starts
        assert!(created.is_ok());
        assert_eq!(recorded.unsubscribed_request_ids(), vec![1]);
        assert!(recorded.data_receivers().is_empty());
        assert!(table.has_upstream_subscription(
            &track_key(),
            PUBLISHER_SESSION,
            UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_closing_while_answering_is_not_registered() {
        // Arrange: the session leaves the repository as it sends SUBSCRIBE_OK
        let cell = Arc::new(OnceLock::<Arc<tokio::sync::Mutex<SessionRepository>>>::new());
        let closing_cell = cell.clone();
        let closing_session = mock_session_answering_subscribe(move || {
            let repository = closing_cell.get().unwrap();
            repository
                .try_lock()
                .unwrap()
                .remove(DEAD_PUBLISHER_SESSION);
            moqt::object::ContentExists::False
        });
        let repository = session_repository_with_sessions(
            vec![(DEAD_PUBLISHER_SESSION, closing_session)],
            VerifiedToken::full_access(),
        )
        .await;
        let _ = cell.set(repository.clone());

        // Act
        let (table, created) = subscribe_through(repository, vec![DEAD_PUBLISHER_SESSION]).await;

        // Assert
        assert!(matches!(
            created,
            Err(CreationFailure {
                code: RequestErrorCode::TrackDoesNotExist,
                ..
            })
        ));
        assert!(table.track_state(&TrackKey::new("ns", "track")).is_none());
    }

    #[tokio::test]
    async fn burst_during_upstream_subscribe_does_not_shift_the_largest() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "video");

        let largest = create_upstream_and_resolve_largest(
            cache_store,
            track_key,
            moqt::object::ContentExists::False,
            true,
        )
        .await;

        assert_eq!(
            largest, None,
            "objects that landed during the upstream SUBSCRIBE must not shift the largest"
        );
    }

    #[tokio::test]
    async fn pre_subscribe_snapshot_keeps_stale_cache_of_rejoined_publisher() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "catalog");
        let cache = cache_store.get_or_create(&track_key);
        for group_id in 0..=4 {
            append_one_object(&cache, group_id);
        }

        let largest = create_upstream_and_resolve_largest(
            cache_store,
            track_key,
            moqt::object::ContentExists::False,
            false,
        )
        .await;

        assert_eq!(
            largest,
            Some(moqt::object::Location {
                group_id: 4,
                object_id: 0,
            })
        );
    }

    #[tokio::test]
    async fn subscribe_ok_largest_wins_when_ahead_of_cache() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "catalog");
        let cache = cache_store.get_or_create(&track_key);
        append_one_object(&cache, 3);

        let largest = create_upstream_and_resolve_largest(
            cache_store,
            track_key,
            moqt::object::ContentExists::True {
                location: moqt::object::Location {
                    group_id: 10,
                    object_id: 0,
                },
            },
            false,
        )
        .await;

        assert_eq!(
            largest,
            Some(moqt::object::Location {
                group_id: 10,
                object_id: 0,
            })
        );
    }

    #[tokio::test]
    async fn a_subscribe_joining_the_upstream_takes_its_subscribe_ok_largest() {
        // Arrange: SUBSCRIBE_OK reports Largest {7, 0}; the cache is empty
        let location = moqt::object::Location {
            group_id: 7,
            object_id: 0,
        };

        // Act
        let largest_locations = resolve_largest_per_subscribe(
            Arc::new(TrackCacheStore::new()),
            TrackKey::new("ns", "catalog"),
            moqt::object::ContentExists::True { location },
            false,
            2,
        )
        .await;

        // Assert: both the creating and the joining SUBSCRIBE take it
        assert_eq!(largest_locations, vec![Some(location), Some(location)]);
    }

    async fn accept_downstream(
        table: &InMemoryLocalPubSubDirectory,
        handler: &MockSubscribeHandler,
    ) -> Acceptance {
        Subscribe
            .accept_downstream_subscription(
                2,
                SessionPeer::Client,
                track_key(),
                upstream_track(ProducerOrigin::Subscribe),
                None,
                table,
                &ControlMessageForwarder {
                    repository: Arc::new(tokio::sync::Mutex::new(detached_session_repository())),
                },
                &Arc::new(TrackCacheStore::new()),
                handler,
            )
            .await
    }

    fn assert_unanswered(handler: &MockSubscribeHandler) {
        assert!(handler.subscribe_errors.lock().unwrap().is_empty());
        assert_eq!(*handler.subscribe_ok_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn upstream_removed_before_registration_leaves_the_subscribe_unanswered() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let handler = MockSubscribeHandler::default();

        // Act
        let acceptance = accept_downstream(&table, &handler).await;

        // Assert
        assert!(matches!(acceptance, Acceptance::UpstreamGone));
        assert_unanswered(&handler);
    }

    #[tokio::test]
    async fn a_subscribe_whose_upstream_ends_while_it_is_accepted_is_answered_on_a_new_upstream() {
        // Arrange: the upstream ends between the SUBSCRIBE finding it and registering on it,
        // as when the last other subscriber leaves; the publisher answers a new SUBSCRIBE
        const SUBSCRIBER_SESSION: SessionId = 2;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(PUBLISHER_SESSION, "ns".to_string(), SessionPeer::Client);
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            producer(ProducerOrigin::Subscribe),
        );
        let repository = session_repository_with_sessions(
            vec![
                (
                    PUBLISHER_SESSION,
                    mock_session_answering_subscribe(|| moqt::object::ContentExists::False),
                ),
                (
                    SUBSCRIBER_SESSION,
                    Box::new(MockUpstreamSession::new(RecordedControlMessages::default())),
                ),
            ],
            VerifiedToken::full_access(),
        )
        .await;
        let resolver = UpstreamPublisherResolver::new(
            Arc::new(NoopRelayRouteRegistry),
            Arc::new(InterRelayConnectionManager::new(
                repository.clone(),
                "unused-relay-token".to_string(),
            )),
        );
        let upstream_ended = Arc::new(AtomicBool::new(false));
        let handler = MockSubscribeHandler {
            on_allocate_track_alias: Some(Arc::new({
                let table = table.clone();
                let upstream_ended = upstream_ended.clone();
                move || {
                    if !upstream_ended.swap(true, Ordering::SeqCst) {
                        table.remove_upstream_track(&track_key());
                    }
                }
            })),
            ..MockSubscribeHandler::default()
        };

        // Act
        Subscribe
            .handle(
                SUBSCRIBER_SESSION,
                &table,
                &ControlMessageForwarder { repository },
                &Arc::new(resolver),
                &Arc::new(TrackCacheStore::new()),
                Box::new(handler.clone()),
            )
            .await;

        // Assert
        assert!(handler.subscribe_errors.lock().unwrap().is_empty());
        assert_eq!(*handler.subscribe_ok_count.lock().unwrap(), 1);
        assert!(table.track_state(&track_key()).is_some());
    }
}

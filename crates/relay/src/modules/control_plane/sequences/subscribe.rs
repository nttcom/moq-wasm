use moqt::RequestErrorCode;
use std::sync::Arc;

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        upstream_creation_serializer::UpstreamCreationSerializer,
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressStartRequest},
        ingress::ingress_coordinator::{IngressCommand, IngressStartRequest},
    },
    domain::{
        delivery_control::DeliveryControl,
        pub_sub_directory::{
            InMemoryLocalPubSubDirectory,
            entry::{self, UpstreamTrack},
        },
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::handler::subscribe::SubscribeHandler,
};

use moqt::object::ContentExists;
use tracing::Span;

pub(crate) mod upstream_join_task;

use upstream_join_task::{
    UpstreamJoin, UpstreamJoinDeps, UpstreamJoinTask, next_answered, send_upstream_subscribes,
    subscribe_initiated,
};

pub(crate) struct Subscribe;

/// Another session's cleanup can remove the upstream subscription between
/// finding it and registering on it; each attempt finds or creates it again.
const UPSTREAM_ATTEMPTS: usize = 3;

enum Acceptance {
    Answered,
    UpstreamGone,
}

/// draft-18 §2.4.2: once latched, the whole track is malformed.
pub(super) fn is_latched_malformed(cache_store: &TrackCacheStore, track_key: &TrackKey) -> bool {
    cache_store
        .get(track_key)
        .is_some_and(|cache| cache.is_malformed())
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
    upstream_track: &UpstreamTrack,
) -> Option<moqt::object::Location> {
    let upstream_largest = match upstream_track.content_exists() {
        ContentExists::True { location } => Some(location),
        ContentExists::False => None,
    };
    cached_largest(cache_store, track_key).max(upstream_largest)
}

/// `late_publishers` are the upstream SUBSCRIBEs still unanswered when the
/// first publisher answered; they join the track once the downstream
/// subscription that asked for it is registered, so the track wants them.
struct UpstreamTrackAccess {
    track_key: TrackKey,
    upstream_track: UpstreamTrack,
    largest_location: Option<moqt::object::Location>,
    late_publishers: Option<UpstreamJoin>,
}

enum UpstreamSubscriptionError {
    PublisherNotFound,
    SubscribeFailed(anyhow::Error),
    IngressStartFailed,
}

impl UpstreamSubscriptionError {
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
            Self::IngressStartFailed => (
                RequestErrorCode::InternalError,
                "Failed to start upstream ingress.".to_string(),
            ),
        }
    }
}

impl Subscribe {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        cache_store: &Arc<TrackCacheStore>,
        upstream_serializer: &UpstreamCreationSerializer,
        handler: Box<dyn SubscribeHandler>,
    ) {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            "SequenceHandler::subscribe"
        );
        if is_latched_malformed(cache_store, &TrackKey::new(track_namespace, track_name)) {
            self.response_malformed(handler.as_ref()).await;
            return;
        }
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
                    ingress_sender,
                    upstream_publisher_resolver,
                    upstream_serializer,
                    cache_store,
                )
                .await
            {
                Ok(upstream_subscription) => upstream_subscription,
                Err(err) => {
                    tracing::warn!(
                        session_id = %session_id,
                        track_namespace = %track_namespace,
                        track_name = %track_name,
                        "failed to get or create upstream subscription"
                    );
                    let (code, reason_phrase) = err.subscribe_error_response();
                    if let Err(send_error) = self
                        .response_error(handler.as_ref(), code, reason_phrase)
                        .await
                    {
                        tracing::error!(
                            subscribe_id = handler.subscribe_id(),
                            track_namespace = %track_namespace,
                            track_name = %track_name,
                            error = ?send_error,
                            "failed to send SUBSCRIBE_ERROR"
                        );
                    }
                    return;
                }
            };

            let acceptance = self
                .accept_downstream_subscription(
                    session_id,
                    requester,
                    access.track_key,
                    access.upstream_track,
                    access.largest_location,
                    table,
                    egress_sender,
                    cache_store,
                    handler.as_ref(),
                )
                .await;
            if let Some(late_publishers) = access.late_publishers {
                let _upstream_join = UpstreamJoinTask::run(
                    late_publishers,
                    UpstreamJoinDeps {
                        table: table.clone(),
                        forwarder: forwarder.clone(),
                        ingress_sender: ingress_sender.clone(),
                        cache_store: cache_store.clone(),
                    },
                );
            }
            if let Acceptance::Answered = acceptance {
                return;
            }
            tracing::info!(
                subscribe_id = handler.subscribe_id(),
                track_namespace = %track_namespace,
                track_name = %track_name,
                "upstream subscription ended before the downstream subscription was accepted; retrying"
            );
        }
        self.response_track_gone(handler.as_ref()).await;
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "info",
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
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        upstream_serializer: &UpstreamCreationSerializer,
        cache_store: &Arc<TrackCacheStore>,
    ) -> Result<UpstreamTrackAccess, UpstreamSubscriptionError> {
        let track_key = TrackKey::new(track_namespace, track_name);
        // Fast path: cache hit without acquiring the per-track lock.
        if let Some(upstream_track) = table.get_upstream_track(&track_key) {
            let largest_location = existing_track_largest(cache_store, &track_key, &upstream_track);
            return Ok(UpstreamTrackAccess {
                track_key,
                upstream_track,
                largest_location,
                late_publishers: None,
            });
        }

        // Cache miss: acquire the per-track lock so that concurrent tasks for
        // the same track serialise here and only one of them calls
        // create_upstream_subscription.
        let _guard = upstream_serializer.lock(track_namespace, track_name).await;

        // Re-check after acquiring the lock: a sibling task may have created
        // and registered the upstream subscription while we were waiting.
        if let Some(upstream_track) = table.get_upstream_track(&track_key) {
            tracing::debug!(
                track_namespace = %track_namespace,
                track_name = %track_name,
                "upstream subscription found after serializer lock (joined existing)"
            );
            let largest_location = existing_track_largest(cache_store, &track_key, &upstream_track);
            return Ok(UpstreamTrackAccess {
                track_key,
                upstream_track,
                largest_location,
                late_publishers: None,
            });
        }

        self.create_upstream_subscription(
            session_id,
            requester,
            track_namespace,
            track_name,
            table,
            forwarder,
            ingress_sender,
            upstream_publisher_resolver,
            cache_store,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.create_upstream_subscription",
        skip_all,
        fields(
            session_id = %session_id,
            track_namespace = %track_namespace,
            track_name = %track_name
        )
    )]
    async fn create_upstream_subscription(
        &self,
        session_id: SessionId,
        requester: SessionPeer,
        track_namespace: &str,
        track_name: &str,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        cache_store: &Arc<TrackCacheStore>,
    ) -> Result<UpstreamTrackAccess, UpstreamSubscriptionError> {
        if requester == SessionPeer::Client {
            upstream_publisher_resolver
                .watched_namespace_routes
                .watch(track_namespace)
                .await;
        }
        let publishers = upstream_publisher_resolver
            .resolve(table, track_namespace, track_name, requester)
            .await;
        if publishers.is_empty() {
            return Err(UpstreamSubscriptionError::PublisherNotFound);
        }

        let track_key = TrackKey::new(track_namespace, track_name);
        let cache_before_subscribe = cached_largest(cache_store, &track_key);

        // draft-14 §8.4
        let mut pending = send_upstream_subscribes(
            upstream_publisher_resolver,
            forwarder,
            &track_key,
            publishers,
        );
        let mut last_error = None;
        let (pub_session_id, publisher_peer, subscription) = loop {
            let Some(answered) = next_answered(&mut pending).await else {
                return Err(UpstreamSubscriptionError::SubscribeFailed(
                    last_error.unwrap_or_else(|| {
                        anyhow::anyhow!("no upstream publisher could be reached")
                    }),
                ));
            };
            match answered.subscribed {
                Ok(subscription) => {
                    break (
                        answered.publisher_session_id,
                        answered.publisher_peer,
                        subscription,
                    );
                }
                Err(error) if super::malformed_track::refused_as_malformed(&error) => {
                    tracing::warn!(
                        %error,
                        pub_session_id = %answered.publisher_session_id,
                        track_namespace = %track_namespace,
                        track_name = %track_name,
                        "upstream SUBSCRIBE refused the track as malformed; latching it"
                    );
                    cache_store.get_or_create(&track_key).mark_malformed();
                    return Err(UpstreamSubscriptionError::SubscribeFailed(error));
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        pub_session_id = %answered.publisher_session_id,
                        track_namespace = %track_namespace,
                        track_name = %track_name,
                        "upstream SUBSCRIBE failed"
                    );
                    last_error = Some(error);
                }
            }
        };
        tracing::info!(
            pub_session_id = %pub_session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            track_alias = subscription.track_alias(),
            expires = subscription.expires().unwrap_or(0),
            "upstream subscribe ok received"
        );

        let active_upstream = subscribe_initiated(&subscription, publisher_peer);

        if !super::start_ingress(
            ingress_sender,
            IngressStartRequest {
                subscriber_session_id: session_id,
                publisher_session_id: pub_session_id,
                track_key: track_key.clone(),
                subscription,
                parent_span: Span::current(),
            },
        )
        .await
        {
            return Err(UpstreamSubscriptionError::IngressStartFailed);
        }
        table.register_upstream_subscription(
            track_key.clone(),
            pub_session_id,
            active_upstream.clone(),
        );
        tracing::info!(
            pub_session_id = %pub_session_id,
            track_namespace = %track_namespace,
            track_name = %track_name,
            "upstream subscription registered"
        );
        let late_publishers = (!pending.is_empty()).then(|| UpstreamJoin {
            track_key: track_key.clone(),
            subscriber_session_id: session_id,
            pending,
        });

        let upstream_largest = match active_upstream.content_exists {
            ContentExists::True { location } => Some(location),
            ContentExists::False => None,
        };
        let subscribe_time_largest = upstream_largest.max(cache_before_subscribe);
        let mut upstream_track = UpstreamTrack::default();
        upstream_track
            .subscriptions
            .insert(pub_session_id, active_upstream);
        Ok(UpstreamTrackAccess {
            track_key,
            upstream_track,
            largest_location: subscribe_time_largest,
            late_publishers,
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        level = "info",
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
        upstream_track: UpstreamTrack,
        largest_location: Option<moqt::object::Location>,
        table: &InMemoryLocalPubSubDirectory,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        cache_store: &Arc<TrackCacheStore>,
        handler: &dyn SubscribeHandler,
    ) -> Acceptance {
        // The track may have been latched while the upstream SUBSCRIBE was
        // in flight.
        if is_latched_malformed(cache_store, &track_key) {
            self.response_malformed(handler).await;
            return Acceptance::Answered;
        }
        let track_properties = upstream_track.track_properties();
        if !handler.understands(&track_properties) {
            let _ = self
                .response_error(
                    handler,
                    RequestErrorCode::UnsupportedExtension,
                    "mandatory track property not understood".to_string(),
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
            entry::DownstreamSubscription {
                track_key: track_key.clone(),
                filter_type,
                joining_location: largest_location,
            },
            DeliveryControl::new(handler.forward(), &filter_type, largest_location),
        ) else {
            tracing::debug!(
                subscribe_id = handler.subscribe_id(),
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                "upstream subscription removed before the downstream subscription was registered"
            );
            return Acceptance::UpstreamGone;
        };

        let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
        let (subscribe_ok_sender, subscribe_ok_receiver) = tokio::sync::oneshot::channel();
        if egress_sender
            .send(EgressCommand::StartReader(Box::new(EgressStartRequest {
                subscriber_session_id: session_id,
                downstream_subscribe_id: handler.subscribe_id(),
                track_key: track_key.clone(),
                downstream_subscription,
                parent_span: Span::current(),
                ready_sender,
                runner_signals,
                subscribe_ok_receiver,
                largest_location,
            })))
            .await
            .is_err()
        {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                "failed to send EgressStartRequest"
            );
            return Acceptance::Answered;
        }
        match ready_receiver.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(
                    ?error,
                    subscribe_id = handler.subscribe_id(),
                    "failed to start egress runner"
                );
                return Acceptance::Answered;
            }
            Err(_) => {
                tracing::debug!(
                    subscribe_id = handler.subscribe_id(),
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
                track_properties,
            )
            .await
            .is_err()
        {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                subscriber_track_alias = subscriber_track_alias,
                "failed to send SUBSCRIBE_OK"
            );
            return Acceptance::Answered;
        }
        let _ = subscribe_ok_sender.send(());
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_key.track_namespace,
            track_name = %track_key.track_name,
            subscriber_track_alias = subscriber_track_alias,
            "downstream subscribe ok sent"
        );
        Acceptance::Answered
    }

    async fn response_malformed(&self, handler: &dyn SubscribeHandler) {
        let _ = self
            .response_error(
                handler,
                RequestErrorCode::MalformedTrack,
                "malformed track".to_string(),
            )
            .await;
    }

    async fn response_track_gone(&self, handler: &dyn SubscribeHandler) {
        let _ = self
            .response_error(
                handler,
                RequestErrorCode::TrackDoesNotExist,
                "track is no longer available".to_string(),
            )
            .await;
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.response_error",
        skip_all
    )]
    async fn response_error(
        &self,
        handler: &dyn SubscribeHandler,
        code: RequestErrorCode,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::warn!(
            subscribe_id = handler.subscribe_id(),
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
    use super::*;
    use crate::modules::auth::verified_token::VerifiedToken;
    use crate::modules::cascading::inter_relay_connection_manager::InterRelayConnectionManager;
    use crate::modules::cascading::route_registry::NoopRelayRouteRegistry;
    use crate::modules::data_plane::cache::track_cache::TrackCache;
    use crate::modules::domain::{
        pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::UpstreamSubscriptionOrigin},
        session_peer::SessionPeer,
    };
    use crate::modules::session::session_repository::SessionRepository;
    use crate::modules::test_support::directory_fixtures::{
        PUBLISHER_SESSION, active_upstream, local_publisher_resolver, track_key, upstream_carrying,
        upstream_track,
    };
    use crate::modules::test_support::directory_fixtures::{
        delivery_control, downstream_subscription,
    };
    use crate::modules::test_support::mock_session::{
        MockSubscribeHandler, mock_session_answering_subscribe,
        mock_session_never_answering_subscribe, mock_session_refusing_subscribe,
        recorded_session_answering_subscribe, session_repository_with_session,
        session_repository_with_sessions,
    };
    use crate::modules::test_support::relay_harness::fixtures::cached_object::insert_closed_group;
    use crate::modules::test_support::relay_harness::fixtures::subscription::unknown_track_properties;

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
        let serializer = UpstreamCreationSerializer::default();
        let (ingress_sender, _ingress_receiver) = tokio::sync::mpsc::channel(4);

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
                    &ingress_sender,
                    &resolver,
                    &serializer,
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
        cache_store: &Arc<TrackCacheStore>,
    ) -> (
        Arc<InMemoryLocalPubSubDirectory>,
        Result<UpstreamTrackAccess, UpstreamSubscriptionError>,
    ) {
        const SUBSCRIBER_SESSION: SessionId = 2;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        for (publisher_session_id, _) in &publishers {
            table.register_publish_namespace(
                *publisher_session_id,
                "ns".to_string(),
                SessionPeer::Client,
            );
        }
        let repository =
            session_repository_with_sessions(publishers, VerifiedToken::full_access()).await;
        let forwarder = ControlMessageForwarder { repository };
        let resolver = Arc::new(local_publisher_resolver());
        let (ingress_sender, mut ingress_receiver) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move { while ingress_receiver.recv().await.is_some() {} });
        let mut created = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            Subscribe.get_or_create_upstream_subscription(
                SUBSCRIBER_SESSION,
                SessionPeer::Client,
                "ns",
                "track",
                &table,
                &forwarder,
                &ingress_sender,
                &resolver,
                &UpstreamCreationSerializer::default(),
                cache_store,
            ),
        )
        .await
        .expect("the SUBSCRIBE must not wait for a publisher that never answers");
        if let Ok(access) = &mut created {
            table
                .register_downstream_subscription(
                    SUBSCRIBER_SESSION,
                    1,
                    SessionPeer::Client,
                    downstream_subscription(access.track_key.clone(), None),
                    delivery_control(true),
                )
                .unwrap();
            if let Some(late_publishers) = access.late_publishers.take() {
                let _upstream_join = UpstreamJoinTask::run(
                    late_publishers,
                    UpstreamJoinDeps {
                        table: table.clone(),
                        forwarder,
                        ingress_sender,
                        cache_store: cache_store.clone(),
                    },
                );
            }
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
                .get_upstream_track(&track_key)
                .map(|track| track.subscriptions.into_keys().collect::<Vec<_>>())
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
        let (table, created) =
            subscribe_to_publishers(publishers, &Arc::new(TrackCacheStore::new())).await;

        // Assert
        assert!(created.is_ok());
        assert_eq!(
            wait_track_publishers(&table, vec![NEW_PUBLISHER_SESSION]).await,
            vec![NEW_PUBLISHER_SESSION]
        );
    }

    #[tokio::test]
    async fn a_subscribe_ok_refused_as_malformed_latches_the_track_and_answers_malformed_track() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let publishers = vec![(
            DEAD_PUBLISHER_SESSION,
            mock_session_refusing_subscribe(RequestErrorCode::MalformedTrack),
        )];

        // Act
        let (_table, created) = subscribe_to_publishers(publishers, &cache_store).await;

        // Assert: draft-18 §2.4.2, §12.7
        let Err(error) = created else {
            panic!("a malformed track must not be subscribed");
        };
        assert_eq!(
            error.subscribe_error_response().0,
            RequestErrorCode::MalformedTrack
        );
        assert!(
            cache_store
                .get(&TrackKey::new("ns", "track"))
                .is_some_and(|cache| cache.is_malformed())
        );
    }

    #[tokio::test]
    async fn a_malformed_refusal_ends_the_subscribe_without_waiting_for_other_publishers() {
        // Arrange
        let publishers = vec![
            (
                DEAD_PUBLISHER_SESSION,
                mock_session_refusing_subscribe(RequestErrorCode::MalformedTrack),
            ),
            (
                NEW_PUBLISHER_SESSION,
                mock_session_never_answering_subscribe(),
            ),
        ];

        // Act
        let (_table, created) =
            subscribe_to_publishers(publishers, &Arc::new(TrackCacheStore::new())).await;

        // Assert: draft-18 §2.4.2, the track is malformed whoever else publishes it
        assert!(matches!(
            created
                .map(|_| ())
                .map_err(|error| error.subscribe_error_response().0),
            Err(RequestErrorCode::MalformedTrack)
        ));
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
        let (table, created) =
            subscribe_to_publishers(publishers, &Arc::new(TrackCacheStore::new())).await;

        // Assert
        assert!(created.is_ok());
        assert_eq!(
            wait_track_publishers(&table, vec![DEAD_PUBLISHER_SESSION, NEW_PUBLISHER_SESSION])
                .await,
            vec![DEAD_PUBLISHER_SESSION, NEW_PUBLISHER_SESSION]
        );
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
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: &MockSubscribeHandler,
    ) -> Acceptance {
        accept_downstream_of(
            upstream_track(UpstreamSubscriptionOrigin::Subscribe),
            table,
            egress_sender,
            handler,
            &Arc::new(TrackCacheStore::new()),
        )
        .await
    }

    async fn accept_downstream_of(
        upstream_track: UpstreamTrack,
        table: &InMemoryLocalPubSubDirectory,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: &MockSubscribeHandler,
        cache_store: &Arc<TrackCacheStore>,
    ) -> Acceptance {
        Subscribe
            .accept_downstream_subscription(
                2,
                SessionPeer::Client,
                track_key(),
                upstream_track,
                None,
                table,
                egress_sender,
                cache_store,
                handler,
            )
            .await
    }

    #[tokio::test]
    async fn a_subscribe_to_a_latched_track_is_answered_malformed_track() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );
        let cache_store = Arc::new(TrackCacheStore::new());
        cache_store.get_or_create(&track_key()).mark_malformed();
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(4);
        let handler = MockSubscribeHandler::default();

        // Act
        let acceptance = accept_downstream_of(
            upstream_track(UpstreamSubscriptionOrigin::Subscribe),
            &table,
            &egress_sender,
            &handler,
            &cache_store,
        )
        .await;

        // Assert: draft-18 §2.4.2, the track is malformed, not missing
        assert!(matches!(acceptance, Acceptance::Answered));
        assert_eq!(
            *handler.subscribe_errors.lock().unwrap(),
            vec![RequestErrorCode::MalformedTrack]
        );
        assert!(egress_receiver.try_recv().is_err());
    }

    fn assert_unanswered(handler: &MockSubscribeHandler) {
        assert!(handler.subscribe_errors.lock().unwrap().is_empty());
        assert!(handler.subscribe_oks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn upstream_removed_before_registration_leaves_the_subscribe_unanswered() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(4);
        let handler = MockSubscribeHandler::default();

        // Act
        let acceptance = accept_downstream(&table, &egress_sender, &handler).await;

        // Assert
        assert!(matches!(acceptance, Acceptance::UpstreamGone));
        assert_unanswered(&handler);
        assert!(egress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn registration_removed_before_runner_readiness_leaves_the_subscribe_unanswered() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(4);
        let concurrent_cleanup = tokio::spawn({
            let table = table.clone();
            async move {
                if let Some(EgressCommand::StartReader(request)) = egress_receiver.recv().await {
                    table.remove_downstream_subscription(
                        request.subscriber_session_id,
                        request.downstream_subscribe_id,
                    );
                }
            }
        });
        let handler = MockSubscribeHandler::default();

        // Act
        let acceptance = accept_downstream(&table, &egress_sender, &handler).await;

        // Assert
        concurrent_cleanup.await.unwrap();
        assert!(matches!(acceptance, Acceptance::UpstreamGone));
        assert_unanswered(&handler);
    }

    #[tokio::test]
    async fn subscribe_ok_carries_the_upstream_track_properties() {
        // Arrange
        let track_properties = moqt::TrackProperties {
            max_cache_duration: Some(2_000),
            ..unknown_track_properties(1)
        };
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            upstream_carrying(track_properties.clone()),
        );
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(4);
        let egress = tokio::spawn(async move {
            if let Some(EgressCommand::StartReader(request)) = egress_receiver.recv().await {
                let _ = request.ready_sender.send(Ok(()));
            }
        });
        let handler = MockSubscribeHandler::default();

        // Act
        accept_downstream_of(
            table.get_upstream_track(&track_key()).unwrap(),
            &table,
            &egress_sender,
            &handler,
            &Arc::new(TrackCacheStore::new()),
        )
        .await;

        // Assert
        egress.await.unwrap();
        assert_eq!(
            *handler.subscribe_oks.lock().unwrap(),
            vec![track_properties]
        );
    }

    #[tokio::test]
    async fn a_subscriber_that_cannot_carry_the_track_is_refused_before_its_egress_starts() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(4);
        let egress_started = tokio::spawn(async move {
            let Some(EgressCommand::StartReader(request)) = egress_receiver.recv().await else {
                return false;
            };
            let _ = request.ready_sender.send(Ok(()));
            true
        });
        let handler = MockSubscribeHandler {
            understands_track_properties: false,
            ..Default::default()
        };

        // Act
        let acceptance = accept_downstream(&table, &egress_sender, &handler).await;
        drop(egress_sender);

        // Assert: draft-18 §2.5.1; nothing is registered or started for it
        assert!(matches!(acceptance, Acceptance::Answered));
        assert_eq!(
            *handler.subscribe_errors.lock().unwrap(),
            vec![RequestErrorCode::UnsupportedExtension]
        );
        assert!(!egress_started.await.unwrap());
        assert!(
            table
                .get_downstream_subscription(2, handler.subscribe_id())
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_subscribe_with_forward_off_starts_its_egress_with_forward_off() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(4);
        let egress = tokio::spawn(async move {
            let Some(EgressCommand::StartReader(request)) = egress_receiver.recv().await else {
                panic!("expected the SUBSCRIBE to start an egress runner");
            };
            let forward = request
                .runner_signals
                .delivery_control_receiver
                .borrow()
                .forward;
            let _ = request.ready_sender.send(Ok(()));
            forward
        });
        let handler = MockSubscribeHandler {
            forward: false,
            ..Default::default()
        };

        // Act
        accept_downstream(&table, &egress_sender, &handler).await;

        // Assert
        assert!(!egress.await.unwrap());
        assert_eq!(handler.subscribe_oks.lock().unwrap().len(), 1);
    }

    /// Handles `handler`'s SUBSCRIBE from session 2, the publisher being
    /// `PUBLISHER_SESSION` in `repository`.
    async fn handle_subscribe(
        table: &Arc<InMemoryLocalPubSubDirectory>,
        repository: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        cache_store: &Arc<TrackCacheStore>,
        handler: &MockSubscribeHandler,
    ) {
        let (session_event_sender, _session_event_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let resolver = UpstreamPublisherResolver::new(
            Arc::new(NoopRelayRouteRegistry),
            Arc::new(InterRelayConnectionManager::new(
                repository.clone(),
                session_event_sender,
                "unused-relay-token".to_string(),
            )),
        );
        let (ingress_sender, _ingress_receiver) = tokio::sync::mpsc::channel(4);
        Subscribe
            .handle(
                2,
                &Span::none(),
                table,
                &ControlMessageForwarder { repository },
                &ingress_sender,
                egress_sender,
                &Arc::new(resolver),
                cache_store,
                &UpstreamCreationSerializer::default(),
                Box::new(handler.clone()),
            )
            .await;
    }

    #[tokio::test]
    async fn a_subscribe_to_a_latched_track_sends_no_upstream_subscribe() {
        // Arrange: the latch ended the track's upstream subscriptions; its publisher still announces the namespace
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(PUBLISHER_SESSION, "ns".to_string(), SessionPeer::Client);
        let (publisher, recorded) = recorded_session_answering_subscribe();
        let repository = session_repository_with_session(
            PUBLISHER_SESSION,
            publisher,
            VerifiedToken::full_access(),
        )
        .await;
        let cache_store = Arc::new(TrackCacheStore::new());
        cache_store.get_or_create(&track_key()).mark_malformed();
        let (egress_sender, _egress_receiver) = tokio::sync::mpsc::channel(4);
        let handler = MockSubscribeHandler::default();

        // Act
        handle_subscribe(&table, repository, &egress_sender, &cache_store, &handler).await;

        // Assert: draft-18 §2.4.2, the whole track is malformed, so asking a publisher for it again is pointless
        assert!(recorded.subscribed_track_names().is_empty());
        assert_eq!(
            *handler.subscribe_errors.lock().unwrap(),
            vec![RequestErrorCode::MalformedTrack]
        );
        assert!(table.get_upstream_track(&track_key()).is_none());
    }

    #[tokio::test]
    async fn a_subscribe_whose_upstream_ends_while_it_is_accepted_is_answered_on_a_new_upstream() {
        // Arrange: the first runner start loses its registration, as when the last other subscriber leaves
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(PUBLISHER_SESSION, "ns".to_string(), SessionPeer::Client);
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );
        let repository = session_repository_with_session(
            PUBLISHER_SESSION,
            mock_session_answering_subscribe(|| moqt::object::ContentExists::False),
            VerifiedToken::full_access(),
        )
        .await;
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(4);
        let egress = tokio::spawn({
            let table = table.clone();
            async move {
                let mut started = 0;
                while let Some(EgressCommand::StartReader(request)) = egress_receiver.recv().await {
                    started += 1;
                    if started == 1 {
                        table.remove_downstream_subscription(
                            request.subscriber_session_id,
                            request.downstream_subscribe_id,
                        );
                    } else {
                        let _ = request.ready_sender.send(Ok(()));
                        return started;
                    }
                }
                started
            }
        });
        let handler = MockSubscribeHandler::default();

        // Act
        handle_subscribe(
            &table,
            repository,
            &egress_sender,
            &Arc::new(TrackCacheStore::new()),
            &handler,
        )
        .await;
        drop(egress_sender);

        // Assert
        assert_eq!(egress.await.unwrap(), 2);
        assert!(handler.subscribe_errors.lock().unwrap().is_empty());
        assert_eq!(handler.subscribe_oks.lock().unwrap().len(), 1);
    }
}

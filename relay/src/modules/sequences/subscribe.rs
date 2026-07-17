use std::sync::Arc;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::subscribe::SubscribeHandler,
    enums::{ContentExists, Location, SubscribeErrorCode},
    relay::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressStartRequest},
        ingress::ingress_coordinator::{IngressCommand, IngressStartRequest},
    },
    sequences::{
        tables::table::{
            ActiveUpstreamSubscription, LocalPubSubDirectory, UpstreamSubscriptionKey,
            UpstreamSubscriptionOrigin,
        },
        upstream_serializer::UpstreamCreationSerializer,
    },
    types::{SessionId, TrackKey},
    upstream_publisher_resolver::UpstreamPublisherResolver,
};
use tracing::Span;

pub(crate) struct Subscribe;

/// Where the subscribe-time Largest Object comes from: the upstream
/// SUBSCRIBE_OK, or the local cache when joining an active upstream.
enum LargestObjectSource {
    LocalCache,
    SubscribeOk(Option<moqt::Location>),
}

/// Return the location with the greater `(group_id, object_id)`, treating
/// `None` as "no content" (i.e. smaller than any location).
fn max_location(a: Option<moqt::Location>, b: Option<moqt::Location>) -> Option<moqt::Location> {
    match (a, b) {
        (Some(a), Some(b)) => {
            if (a.group_id, a.object_id) >= (b.group_id, b.object_id) {
                Some(a)
            } else {
                Some(b)
            }
        }
        (some, None) | (None, some) => some,
    }
}

/// Resolve the subscribe-time Largest Object for a downstream subscription,
/// from the upstream SUBSCRIBE_OK and/or the local cache for this track.
///
/// The cache is always consulted, even for a freshly created upstream: a
/// publisher that left and rejoined under the same track leaves its prior
/// objects cached, and ignoring them (when the new upstream reports no content)
/// makes the relay advertise contentExists=false and replay the stale cache
/// from {0,0}. Taking the max also covers a publisher that already had content
/// at SUBSCRIBE_OK time before the cache caught up.
async fn resolve_subscribe_largest(
    largest_source: &LargestObjectSource,
    track_key: &TrackKey,
    cache_store: &TrackCacheStore,
) -> Option<moqt::Location> {
    let cached = match cache_store.get(track_key) {
        Some(cache) => cache.largest_location().await,
        None => None,
    };
    match largest_source {
        LargestObjectSource::LocalCache => cached,
        LargestObjectSource::SubscribeOk(location) => max_location(*location, cached),
    }
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
    fn subscribe_error_response(&self) -> (u64, String) {
        match self {
            Self::PublisherNotFound => (
                SubscribeErrorCode::TrackDoesNotExist as u64,
                "Designated namespace and track name do not exist.".to_string(),
            ),
            Self::SubscribeFailed(error) => {
                if let Some(subscribe_error) = error.downcast_ref::<moqt::wire::RequestError>() {
                    (
                        subscribe_error.error_code,
                        subscribe_error.reason_phrase.clone(),
                    )
                } else if error.downcast_ref::<moqt::RequestTimeoutError>().is_some() {
                    (
                        SubscribeErrorCode::Timeout as u64,
                        "Upstream subscribe timed out".to_string(),
                    )
                } else {
                    (
                        SubscribeErrorCode::InternalError as u64,
                        "Failed to create upstream subscription.".to_string(),
                    )
                }
            }
            Self::IngressStartFailed => (
                SubscribeErrorCode::InternalError as u64,
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
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
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

        let (upstream_key, active_upstream, largest_source) = match self
            .get_or_create_upstream_subscription(
                session_id,
                track_namespace,
                track_name,
                table,
                forwarder,
                ingress_sender,
                upstream_publisher_resolver,
                upstream_serializer,
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

        self.accept_downstream_subscription(
            session_id,
            upstream_key,
            active_upstream,
            largest_source,
            table,
            egress_sender,
            cache_store,
            handler.as_ref(),
        )
        .await;
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
        track_namespace: &str,
        track_name: &str,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
        upstream_serializer: &UpstreamCreationSerializer,
    ) -> Result<
        (
            UpstreamSubscriptionKey,
            ActiveUpstreamSubscription,
            LargestObjectSource,
        ),
        UpstreamSubscriptionError,
    > {
        // Fast path: cache hit without acquiring the per-track lock.
        if let Some((upstream_key, active_upstream)) =
            self.find_active_upstream_subscription(table, track_namespace, track_name)
        {
            return Ok((
                upstream_key,
                active_upstream,
                LargestObjectSource::LocalCache,
            ));
        }

        // Cache miss: acquire the per-track lock so that concurrent tasks for
        // the same track serialise here and only one of them calls
        // create_upstream_subscription.
        let _guard = upstream_serializer.lock(track_namespace, track_name).await;

        // Re-check after acquiring the lock: a sibling task may have created
        // and registered the upstream subscription while we were waiting.
        if let Some((upstream_key, active_upstream)) =
            self.find_active_upstream_subscription(table, track_namespace, track_name)
        {
            tracing::debug!(
                track_namespace = %track_namespace,
                track_name = %track_name,
                "upstream subscription found after serializer lock (joined existing)"
            );
            return Ok((
                upstream_key,
                active_upstream,
                LargestObjectSource::LocalCache,
            ));
        }

        // Still a miss: we are the first task for this track. Create the
        // upstream subscription while holding the guard. The guard is dropped
        // at the end of this scope, after register_upstream_subscription
        // has been called inside create_upstream_subscription.
        let (upstream_key, active_upstream) = self
            .create_upstream_subscription(
                session_id,
                track_namespace,
                track_name,
                table,
                forwarder,
                ingress_sender,
                upstream_publisher_resolver,
            )
            .await?;
        let largest_source =
            LargestObjectSource::SubscribeOk(active_upstream.content_exists.location());

        Ok((upstream_key, active_upstream, largest_source))
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.find_active_upstream_subscription",
        skip_all,
        fields(
            track_namespace = %track_namespace,
            track_name = %track_name
        )
    )]
    fn find_active_upstream_subscription(
        &self,
        table: &dyn LocalPubSubDirectory,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<(UpstreamSubscriptionKey, ActiveUpstreamSubscription)> {
        table
            .find_active_upstream_subscriptions(track_namespace, track_name)
            .into_iter()
            .min_by_key(|publisher| publisher.publisher_session_id)
            .and_then(|upstream_key| {
                let active_upstream = table.get_active_upstream_subscription(
                    upstream_key.publisher_session_id,
                    upstream_key.track_namespace.as_str(),
                    upstream_key.track_name.as_str(),
                )?;
                Some((upstream_key, active_upstream))
            })
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
        track_namespace: &str,
        track_name: &str,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &tokio::sync::mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
    ) -> Result<(UpstreamSubscriptionKey, ActiveUpstreamSubscription), UpstreamSubscriptionError>
    {
        let upstream_key = upstream_publisher_resolver
            .resolve(table, track_namespace, track_name)
            .await
            .map_err(|err| {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to resolve upstream publisher"
                );
                UpstreamSubscriptionError::PublisherNotFound
            })?
            .ok_or(UpstreamSubscriptionError::PublisherNotFound)?;

        let pub_session_id = upstream_key.publisher_session_id;
        let subscription = match forwarder
            .subscribe(
                pub_session_id,
                upstream_key.track_namespace.clone(),
                upstream_key.track_name.clone(),
            )
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                tracing::warn!(
                    %error,
                    pub_session_id = %pub_session_id,
                    track_namespace = %upstream_key.track_namespace,
                    track_name = %upstream_key.track_name,
                    "upstream SUBSCRIBE failed"
                );
                return Err(UpstreamSubscriptionError::SubscribeFailed(error));
            }
        };
        tracing::info!(
            pub_session_id = %pub_session_id,
            track_namespace = %upstream_key.track_namespace,
            track_name = %upstream_key.track_name,
            track_alias = subscription.track_alias(),
            expires = subscription.expires().unwrap_or(0),
            "upstream subscribe ok received"
        );

        let track_key = TrackKey::new(&upstream_key.track_namespace, &upstream_key.track_name);
        let active_upstream = ActiveUpstreamSubscription {
            upstream_request_id: subscription.request_id(),
            track_key,
            expires: subscription.expires(),
            content_exists: subscription.content_exists(),
            downstream_subscriber_count: 0,
            origin: UpstreamSubscriptionOrigin::Subscribe,
        };

        if ingress_sender
            .send(IngressCommand::Start(Box::new(IngressStartRequest {
                subscriber_session_id: session_id,
                publisher_session_id: pub_session_id,
                track_namespace: upstream_key.track_namespace.clone(),
                track_name: upstream_key.track_name.clone(),
                subscription,
                parent_span: Span::current(),
            })))
            .await
            .is_err()
        {
            tracing::error!(
                pub_session_id = %pub_session_id,
                track_namespace = %upstream_key.track_namespace,
                track_name = %upstream_key.track_name,
                "failed to send ingress start request"
            );
            return Err(UpstreamSubscriptionError::IngressStartFailed);
        }
        table.register_upstream_subscription(upstream_key.clone(), active_upstream.clone());
        tracing::info!(
            pub_session_id = %pub_session_id,
            track_namespace = %upstream_key.track_namespace,
            track_name = %upstream_key.track_name,
            "upstream subscription registered"
        );

        Ok((upstream_key, active_upstream))
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
        upstream_key: UpstreamSubscriptionKey,
        active_upstream: ActiveUpstreamSubscription,
        largest_source: LargestObjectSource,
        table: &dyn LocalPubSubDirectory,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        cache_store: &Arc<TrackCacheStore>,
        handler: &dyn SubscribeHandler,
    ) {
        let subscriber_track_alias = handler.allocate_track_alias();

        // Determined here so the Largest Location advertised in SUBSCRIBE_OK
        // and the egress delivery start agree.
        let largest_location =
            resolve_subscribe_largest(&largest_source, &active_upstream.track_key, cache_store)
                .await;
        let content_exists = match &largest_location {
            Some(loc) => ContentExists::True {
                location: Location {
                    group_id: loc.group_id,
                    object_id: loc.object_id,
                },
            },
            None => active_upstream.content_exists.clone(),
        };

        if !table.register_downstream_subscription(
            session_id,
            handler.subscribe_id(),
            upstream_key.clone(),
            largest_location,
        ) {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                track_namespace = %upstream_key.track_namespace,
                track_name = %upstream_key.track_name,
                "failed to register downstream subscription"
            );
            return;
        }

        let (ready_sender, ready_receiver) = tokio::sync::oneshot::channel();
        if egress_sender
            .send(EgressCommand::StartReader(Box::new(EgressStartRequest {
                subscriber_session_id: session_id,
                downstream_subscribe_id: handler.subscribe_id(),
                track_key: active_upstream.track_key,
                track_namespace: upstream_key.track_namespace.clone(),
                track_name: upstream_key.track_name.clone(),
                downstream_subscription: handler.to_downstream_subscription(subscriber_track_alias),
                parent_span: Span::current(),
                ready_sender,
                largest_location,
            })))
            .await
            .is_err()
        {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                track_namespace = %upstream_key.track_namespace,
                track_name = %upstream_key.track_name,
                "failed to send EgressStartRequest"
            );
            return;
        }
        match ready_receiver.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(
                    ?error,
                    subscribe_id = handler.subscribe_id(),
                    "failed to start egress runner"
                );
                return;
            }
            Err(error) => {
                tracing::error!(
                    ?error,
                    subscribe_id = handler.subscribe_id(),
                    "egress runner readiness dropped"
                );
                return;
            }
        }

        if handler
            .ok_with_track_alias(
                subscriber_track_alias,
                active_upstream.expires.unwrap_or(0),
                content_exists,
            )
            .await
            .is_err()
        {
            tracing::error!(
                subscribe_id = handler.subscribe_id(),
                subscriber_track_alias = subscriber_track_alias,
                "failed to send SUBSCRIBE_OK"
            );
            // TODO: send_unsubscribe
            // TODO: close session
            return;
        }
        tracing::info!(
            session_id = %session_id,
            track_namespace = %upstream_key.track_namespace,
            track_name = %upstream_key.track_name,
            subscriber_track_alias = subscriber_track_alias,
            "downstream subscribe ok sent"
        );
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.subscribe.response_error",
        skip_all
    )]
    async fn response_error(
        &self,
        handler: &dyn SubscribeHandler,
        code: u64,
        reason_phrase: String,
    ) -> Result<(), moqt::TransportSendError> {
        let track_namespace = handler.track_namespace();
        let track_name = handler.track_name();
        tracing::warn!(
            subscribe_id = handler.subscribe_id(),
            track_namespace = %track_namespace,
            track_name = %track_name,
            error_code = code,
            reason_phrase = %reason_phrase,
            "Sending `SUBSCRIBE_ERROR`"
        );
        handler.error(code, reason_phrase).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::core::data_object::DataObject;
    use crate::modules::relay::cache::track_cache::TrackCache;
    use crate::modules::relay::types::StreamSubgroupId;
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField};

    fn make_header() -> DataObject {
        DataObject::SubgroupHeader(SubgroupHeader::new(
            0,
            0,
            SubgroupId::Value(0),
            0,
            false,
            false,
        ))
    }

    fn make_object() -> DataObject {
        let message_type =
            SubgroupHeader::new(0, 0, SubgroupId::Value(0), 0, false, false).message_type;
        DataObject::SubgroupObject(SubgroupObjectField {
            message_type,
            object_id_delta: 0,
            extension_headers: ExtensionHeaders::default(),
            subgroup_object: SubgroupObject::new_payload(Bytes::from(vec![])),
        })
    }

    // Append a single object (id 0) to `group_id` so the cache reports it as content.
    async fn append_one_object(cache: &TrackCache, group_id: u64) {
        let subgroup = StreamSubgroupId::Value(0);
        cache
            .append_live_stream_object(group_id, &subgroup, None, make_header())
            .await;
        cache
            .append_live_stream_object(group_id, &subgroup, Some(0), make_object())
            .await;
    }

    #[test]
    fn upstream_timeout_maps_to_subscribe_error_timeout() {
        let error = UpstreamSubscriptionError::SubscribeFailed(anyhow::Error::new(
            moqt::RequestTimeoutError,
        ));
        let (code, _reason) = error.subscribe_error_response();
        assert_eq!(code, SubscribeErrorCode::Timeout as u64);
    }

    #[test]
    fn upstream_subscribe_error_code_is_relayed_verbatim() {
        let error = UpstreamSubscriptionError::SubscribeFailed(anyhow::Error::new(
            moqt::wire::RequestError {
                request_id: 7,
                error_code: 0x1,
                reason_phrase: "unauthorized".to_string(),
            },
        ));
        let (code, reason) = error.subscribe_error_response();
        assert_eq!(code, 0x1);
        assert_eq!(reason, "unauthorized");
    }

    #[test]
    fn publisher_not_found_maps_to_track_does_not_exist() {
        let (code, _reason) =
            UpstreamSubscriptionError::PublisherNotFound.subscribe_error_response();
        assert_eq!(code, SubscribeErrorCode::TrackDoesNotExist as u64);
    }

    // A publisher that left a catalog cache (groups 0..=4) then reconnected as a
    // fresh upstream reporting no content (SubscribeOk(None)) must still resolve
    // the cached Largest Object (group 4), not None. Returning None makes the
    // relay advertise contentExists=false and replay the stale cache from {0,0}
    // out of order, leaving the subscriber on an old catalog version.
    #[tokio::test]
    async fn subscribe_largest_uses_cache_when_fresh_upstream_reports_no_content() {
        let cache_store = TrackCacheStore::new();
        let track_key = TrackKey::new("e2e-room/bob", "catalog");
        let cache = cache_store.get_or_create(&track_key);
        for group_id in 0..=4 {
            append_one_object(&cache, group_id).await;
        }

        let largest = resolve_subscribe_largest(
            &LargestObjectSource::SubscribeOk(None),
            &track_key,
            &cache_store,
        )
        .await;

        assert_eq!(
            largest,
            Some(moqt::Location {
                group_id: 4,
                object_id: 0,
            })
        );
    }

    // When the publisher already had content at SUBSCRIBE_OK time but the cache
    // has not caught up yet, the SUBSCRIBE_OK location is larger and must win.
    #[tokio::test]
    async fn subscribe_largest_prefers_subscribe_ok_when_ahead_of_cache() {
        let cache_store = TrackCacheStore::new();
        let track_key = TrackKey::new("e2e-room/bob", "catalog");
        let cache = cache_store.get_or_create(&track_key);
        append_one_object(&cache, 3).await;

        let largest = resolve_subscribe_largest(
            &LargestObjectSource::SubscribeOk(Some(moqt::Location {
                group_id: 10,
                object_id: 0,
            })),
            &track_key,
            &cache_store,
        )
        .await;

        assert_eq!(
            largest,
            Some(moqt::Location {
                group_id: 10,
                object_id: 0,
            })
        );
    }
}

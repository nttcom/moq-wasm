use std::sync::Arc;

use tracing::Span;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::fetch::FetchHandler,
    enums::FetchErrorCode,
    relay::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressFetchRequest},
    },
    sequences::tables::table::LocalPubSubDirectory,
    types::{SessionId, TrackKey},
    upstream_publisher_resolver::UpstreamPublisherResolver,
};

pub(crate) struct Fetch;

/// A Fetch resolved to a concrete track and Object range.
struct ResolvedFetch {
    track_key: TrackKey,
    start_location: moqt::Location,
    end_location: moqt::Location,
}

/// A reason a FETCH could not be resolved, mapped to a FETCH_ERROR.
#[derive(Debug)]
enum FetchError {
    TrackNotFound,
    UnknownJoiningRequestId,
    NoObjectsPublished,
}

impl FetchError {
    fn code(&self) -> FetchErrorCode {
        match self {
            Self::TrackNotFound => FetchErrorCode::TrackDoesNotExist,
            Self::UnknownJoiningRequestId => FetchErrorCode::InvalidJoiningRequestId,
            Self::NoObjectsPublished => FetchErrorCode::InvalidRange,
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            Self::TrackNotFound => "Track not found",
            Self::UnknownJoiningRequestId => "Unknown joining request id",
            Self::NoObjectsPublished => "No objects published",
        }
    }
}

impl Fetch {
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.fetch",
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
        cache_store: &Arc<TrackCacheStore>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
        handler: Box<dyn FetchHandler>,
    ) {
        let fetch_type = handler.fetch_type();
        let request_id = handler.request_id();

        // For standalone fetches with a cache miss, try upstream forwarding.
        if let moqt::wire::FetchType::Standalone {
            ref track_namespace,
            ref track_name,
            start_location,
            end_location,
        } = fetch_type
        {
            let track_namespace_str = track_namespace.join("/");
            if cache_store.get(&TrackKey::new(&track_namespace_str, track_name)).is_none() {
                self.handle_upstream_fetch(
                    session_id,
                    table,
                    forwarder,
                    upstream_publisher_resolver,
                    handler,
                    track_namespace.clone(),
                    track_name.clone(),
                    start_location,
                    end_location,
                )
                .await;
                return;
            }
        }

        // Local cache path: resolve and deliver from cache.
        let ResolvedFetch {
            track_key,
            start_location,
            end_location,
        } = match self
            .resolve_fetch(session_id, fetch_type, table, cache_store)
            .await
        {
            Ok(r) => r,
            Err(err) => {
                let _ = handler
                    .error(err.code() as u64, err.reason().to_string())
                    .await;
                return;
            }
        };

        // Send FETCH_OK on the control stream.
        if let Err(e) = handler.ok(false, end_location).await {
            tracing::error!(?e, "Failed to send FETCH_OK");
            return;
        }

        // Delegate data delivery to egress.
        let fetch_request = EgressFetchRequest {
            subscriber_session_id: session_id,
            request_id,
            track_key,
            start_location,
            end_location,
        };
        if let Err(e) = egress_sender
            .send(EgressCommand::StartFetch(fetch_request))
            .await
        {
            tracing::error!(?e, "Failed to send fetch request to egress");
        }
    }

    /// Forward FETCH upstream when the track is not in local cache.
    #[allow(clippy::too_many_arguments)]
    async fn handle_upstream_fetch(
        &self,
        session_id: SessionId,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
        handler: Box<dyn FetchHandler>,
        track_namespace: Vec<String>,
        track_name: String,
        start_location: moqt::Location,
        end_location: moqt::Location,
    ) {
        let track_namespace_str = track_namespace.join("/");
        let request_id = handler.request_id();

        // Resolve upstream publisher via route registry / inter-relay.
        let upstream_key = match upstream_publisher_resolver
            .resolve(table, &track_namespace_str, &track_name)
            .await
        {
            Ok(Some(key)) => key,
            Ok(None) => {
                tracing::warn!(
                    track_namespace = %track_namespace_str,
                    track_name = %track_name,
                    "No upstream publisher found for fetch"
                );
                let _ = handler
                    .error(
                        FetchErrorCode::TrackDoesNotExist as u64,
                        FetchError::TrackNotFound.reason().to_string(),
                    )
                    .await;
                return;
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    track_namespace = %track_namespace_str,
                    track_name = %track_name,
                    "Failed to resolve upstream publisher for fetch"
                );
                let _ = handler
                    .error(
                        FetchErrorCode::InternalError as u64,
                        "Internal relay error".to_string(),
                    )
                    .await;
                return;
            }
        };

        // Forward FETCH to the upstream publisher session.
        let (fetch_handle, mut receiver) = match forwarder
            .fetch(
                upstream_key.publisher_session_id,
                upstream_key.track_namespace.clone(),
                upstream_key.track_name.clone(),
                start_location,
                end_location,
            )
            .await
        {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    pub_session_id = upstream_key.publisher_session_id,
                    track_namespace = %track_namespace_str,
                    track_name = %track_name,
                    "Upstream FETCH failed"
                );
                let _ = handler
                    .error(
                        FetchErrorCode::TrackDoesNotExist as u64,
                        FetchError::TrackNotFound.reason().to_string(),
                    )
                    .await;
                return;
            }
        };

        tracing::info!(
            pub_session_id = upstream_key.publisher_session_id,
            track_namespace = %track_namespace_str,
            track_name = %track_name,
            upstream_request_id = fetch_handle.request_id,
            "Upstream FETCH_OK received; forwarding to downstream"
        );

        // Relay FETCH_OK to the downstream subscriber using upstream's end_location.
        if let Err(e) = handler.ok(fetch_handle.end_of_track, fetch_handle.end_location).await {
            tracing::error!(?e, "Failed to send FETCH_OK to downstream");
            return;
        }

        // Create a fetch sender for the downstream subscriber to relay objects.
        // EgressCommand::StartFetch reads from cache, so we bypass it and create
        // the sender directly via the forwarder.
        let sender = match forwarder.new_fetch_sender(session_id, request_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(?e, "Failed to create fetch sender for upstream relay");
                return;
            }
        };

        // Relay objects from upstream to downstream in a background task.
        tokio::spawn(async move {
            loop {
                match receiver.receive().await {
                    Ok(moqt::Fetch::Header(_)) => {
                        // Header is consumed internally; continue to objects.
                    }
                    Ok(moqt::Fetch::Object(obj)) => {
                        if let Err(e) = sender.send(obj).await {
                            tracing::error!(?e, "Failed to relay fetch object to downstream");
                            break;
                        }
                    }
                    Ok(moqt::Fetch::End) => {
                        if let Err(e) = sender.close().await {
                            tracing::error!(?e, "Failed to close downstream fetch stream");
                        }
                        break;
                    }
                    Err(e) => {
                        tracing::error!(?e, "Error receiving from upstream fetch stream");
                        break;
                    }
                }
            }
        });
    }

    async fn resolve_fetch(
        &self,
        session_id: SessionId,
        fetch_type: moqt::wire::FetchType,
        table: &dyn LocalPubSubDirectory,
        cache_store: &Arc<TrackCacheStore>,
    ) -> Result<ResolvedFetch, FetchError> {
        match fetch_type {
            moqt::wire::FetchType::Standalone {
                track_namespace,
                track_name,
                start_location,
                end_location,
            } => {
                self.resolve_standalone(
                    cache_store,
                    track_namespace,
                    track_name,
                    start_location,
                    end_location,
                )
                .await
            }
            moqt::wire::FetchType::RelativeJoining {
                joining_request_id,
                joining_start,
            } => {
                self.resolve_relative_joining(
                    session_id,
                    joining_request_id,
                    joining_start,
                    table,
                    cache_store,
                )
                .await
            }
            moqt::wire::FetchType::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => {
                self.resolve_absolute_joining(
                    session_id,
                    joining_request_id,
                    joining_start,
                    table,
                    cache_store,
                )
                .await
            }
        }
    }

    async fn resolve_relative_joining(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        joining_start: u64,
        table: &dyn LocalPubSubDirectory,
        cache_store: &TrackCacheStore,
    ) -> Result<ResolvedFetch, FetchError> {
        let (track_key, largest) = self
            .resolve_joined_subscription(session_id, joining_request_id, table, cache_store)
            .await?;
        Ok(ResolvedFetch {
            track_key,
            // Relative: Start = {Largest.Group - n, 0}.
            start_location: moqt::Location {
                group_id: largest.group_id.saturating_sub(joining_start),
                object_id: 0,
            },
            // §9.16.2.1: End = {Largest.Group, Largest.Object + 1}.
            end_location: moqt::Location {
                group_id: largest.group_id,
                object_id: largest.object_id + 1,
            },
        })
    }

    async fn resolve_absolute_joining(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        joining_start: u64,
        table: &dyn LocalPubSubDirectory,
        cache_store: &TrackCacheStore,
    ) -> Result<ResolvedFetch, FetchError> {
        let (track_key, largest) = self
            .resolve_joined_subscription(session_id, joining_request_id, table, cache_store)
            .await?;
        Ok(ResolvedFetch {
            track_key,
            // Absolute: Start = {n, 0}.
            start_location: moqt::Location {
                group_id: joining_start,
                object_id: 0,
            },
            // §9.16.2.1: End = {Largest.Group, Largest.Object + 1}.
            end_location: moqt::Location {
                group_id: largest.group_id,
                object_id: largest.object_id + 1,
            },
        })
    }

    async fn resolve_standalone(
        &self,
        cache_store: &Arc<TrackCacheStore>,
        track_namespace: Vec<String>,
        track_name: String,
        start_location: moqt::Location,
        end_location: moqt::Location,
    ) -> Result<ResolvedFetch, FetchError> {
        let track_namespace = track_namespace.join("/");
        let track_key = TrackKey::new(&track_namespace, &track_name);
        if cache_store.get(&track_key).is_none() {
            tracing::warn!(track_namespace, track_name, "No cached track for fetch");
            return Err(FetchError::TrackNotFound);
        }
        // FIXME: end_location should reflect what is actually available in cache,
        // not the client-requested end.
        Ok(ResolvedFetch {
            track_key,
            start_location,
            end_location,
        })
    }

    /// Resolves the joined subscription's track and its Largest Location at subscribe time,
    /// shared by Relative and Absolute Joining Fetch.
    ///
    /// When no objects existed at subscribe time (`start_location` is `None`), falls back
    /// to the relay's local cache to find the largest object seen so far. This handles the
    /// cross-relay case where a subscriber joins before the publisher has produced any
    /// objects, and objects arrive later via a live upstream subscription.
    async fn resolve_joined_subscription(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        table: &dyn LocalPubSubDirectory,
        cache_store: &TrackCacheStore,
    ) -> Result<(TrackKey, moqt::Location), FetchError> {
        // TODO: validate the joined Subscribe has Filter Type Largest Object;
        // otherwise close the session with PROTOCOL_VIOLATION (§9.16.2).

        let Some(downstream_sub) =
            table.get_downstream_subscription(session_id, joining_request_id)
        else {
            tracing::warn!(
                joining_request_id,
                "Joining fetch references unknown subscription"
            );
            return Err(FetchError::UnknownJoiningRequestId);
        };

        let Some(active_upstream) = table.get_active_upstream_subscription(
            downstream_sub.upstream_key.publisher_session_id,
            &downstream_sub.upstream_key.track_namespace,
            &downstream_sub.upstream_key.track_name,
        ) else {
            tracing::warn!("Joined subscription has no active upstream subscription");
            return Err(FetchError::TrackNotFound);
        };

        // Use the recorded subscribe-time largest location, falling back to the
        // current cache largest when no objects existed at subscribe time.
        let largest = match downstream_sub.start_location {
            Some(loc) => loc,
            None => {
                // Run async largest_location lookup.
                let cached = match cache_store.get(&active_upstream.track_key) {
                    Some(cache) => cache.largest_location().await,
                    None => None,
                };
                match cached {
                    Some(loc) => {
                        tracing::debug!(
                            track_key = %active_upstream.track_key,
                            group_id = loc.group_id,
                            object_id = loc.object_id,
                            "Joining fetch: no objects at subscribe time; using cache largest"
                        );
                        loc
                    }
                    None => {
                        tracing::warn!("Joining fetch: no objects published at subscribe time");
                        return Err(FetchError::NoObjectsPublished);
                    }
                }
            }
        };

        Ok((active_upstream.track_key, largest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        enums::ContentExists,
        sequences::tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{
                ActiveUpstreamSubscription, UpstreamSubscriptionKey, UpstreamSubscriptionOrigin,
            },
        },
    };

    fn setup_upstream(
        table: &InMemoryLocalPubSubDirectory,
        track_key: TrackKey,
    ) -> UpstreamSubscriptionKey {
        let key = UpstreamSubscriptionKey {
            publisher_session_id: 1,
            track_namespace: "ns".to_string(),
            track_name: "track".to_string(),
        };
        table.register_upstream_subscription(
            key.clone(),
            ActiveUpstreamSubscription {
                upstream_request_id: 1,
                track_key,
                expires: None,
                content_exists: ContentExists::False,
                downstream_subscriber_count: 0,
                origin: UpstreamSubscriptionOrigin::Subscribe,
            },
        );
        key
    }

    #[tokio::test]
    async fn resolve_standalone_track_not_found() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let loc = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let result = Fetch
            .resolve_standalone(
                &cache_store,
                vec!["ns".to_string()],
                "track".to_string(),
                loc,
                loc,
            )
            .await;
        assert!(matches!(result, Err(FetchError::TrackNotFound)));
    }

    #[tokio::test]
    async fn resolve_standalone_returns_resolved() {
        let cache_store = Arc::new(TrackCacheStore::new());
        cache_store.get_or_create(&TrackKey::new("ns", "track"));
        let start = moqt::Location {
            group_id: 1,
            object_id: 2,
        };
        let end = moqt::Location {
            group_id: 3,
            object_id: 4,
        };
        let resolved = Fetch
            .resolve_standalone(
                &cache_store,
                vec!["ns".to_string()],
                "track".to_string(),
                start,
                end,
            )
            .await
            .unwrap();
        assert_eq!(resolved.track_key, TrackKey::new("ns", "track"));
        assert_eq!(resolved.start_location, start);
        assert_eq!(resolved.end_location, end);
    }

    #[tokio::test]
    async fn resolve_joined_subscription_unknown_request_id() {
        let table = InMemoryLocalPubSubDirectory::new();
        let cache_store = TrackCacheStore::new();
        let result = Fetch
            .resolve_joined_subscription(1, 999, &table, &cache_store)
            .await;
        assert!(matches!(result, Err(FetchError::UnknownJoiningRequestId)));
    }

    #[tokio::test]
    async fn resolve_joined_subscription_no_objects_published() {
        let table = InMemoryLocalPubSubDirectory::new();
        let cache_store = TrackCacheStore::new();
        let key = setup_upstream(&table, TrackKey::new("ns", "track"));
        table.register_downstream_subscription(2, 100, key, None);
        // No objects in cache either, so NoObjectsPublished.
        let result = Fetch
            .resolve_joined_subscription(2, 100, &table, &cache_store)
            .await;
        assert!(matches!(result, Err(FetchError::NoObjectsPublished)));
    }

    #[tokio::test]
    async fn resolve_joined_subscription_returns_stored_largest() {
        let table = InMemoryLocalPubSubDirectory::new();
        let cache_store = TrackCacheStore::new();
        let largest = moqt::Location {
            group_id: 10,
            object_id: 5,
        };
        let key = setup_upstream(&table, TrackKey::new("ns", "track"));
        table.register_downstream_subscription(2, 100, key, Some(largest));
        let (track_key, loc) = Fetch
            .resolve_joined_subscription(2, 100, &table, &cache_store)
            .await
            .unwrap();
        assert_eq!(track_key, TrackKey::new("ns", "track"));
        assert_eq!(
            loc,
            moqt::Location {
                group_id: 10,
                object_id: 5
            }
        );
    }
}

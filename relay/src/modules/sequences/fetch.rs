use std::sync::Arc;

use tracing::Span;

use crate::modules::{
    core::handler::fetch::FetchHandler,
    enums::FetchErrorCode,
    relay::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressFetchRequest},
    },
    sequences::tables::table::LocalPubSubDirectory,
    types::{SessionId, TrackKey},
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
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &dyn LocalPubSubDirectory,
        cache_store: &Arc<TrackCacheStore>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        handler: Box<dyn FetchHandler>,
    ) {
        // Resolve the target track and the Object range to deliver.
        let ResolvedFetch {
            track_key,
            start_location,
            end_location,
        } = match self
            .resolve_fetch(session_id, handler.fetch_type(), table, cache_store)
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
            request_id: handler.request_id(),
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
                self.resolve_relative_joining(session_id, joining_request_id, joining_start, table)
                    .await
            }
            moqt::wire::FetchType::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => {
                self.resolve_absolute_joining(session_id, joining_request_id, joining_start, table)
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
    ) -> Result<ResolvedFetch, FetchError> {
        let (track_key, largest) = self
            .resolve_joined_subscription(session_id, joining_request_id, table)
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
    ) -> Result<ResolvedFetch, FetchError> {
        let (track_key, largest) = self
            .resolve_joined_subscription(session_id, joining_request_id, table)
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
        // FIXME: cache miss returns TrackNotFound. draft §9.16 allows forwarding
        // FETCH upstream to fill the gap (MAY); not implemented.
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
    async fn resolve_joined_subscription(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        table: &dyn LocalPubSubDirectory,
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

        let Some(largest) = downstream_sub.start_location else {
            tracing::warn!("Joining fetch: no objects published at subscribe time");
            return Err(FetchError::NoObjectsPublished);
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
        let result = Fetch.resolve_joined_subscription(1, 999, &table).await;
        assert!(matches!(result, Err(FetchError::UnknownJoiningRequestId)));
    }

    #[tokio::test]
    async fn resolve_joined_subscription_no_objects_published() {
        let table = InMemoryLocalPubSubDirectory::new();
        let key = setup_upstream(&table, TrackKey::new("ns", "track"));
        table.register_downstream_subscription(2, 100, key, None);
        let result = Fetch.resolve_joined_subscription(2, 100, &table).await;
        assert!(matches!(result, Err(FetchError::NoObjectsPublished)));
    }

    #[tokio::test]
    async fn resolve_joined_subscription_returns_stored_largest() {
        let table = InMemoryLocalPubSubDirectory::new();
        let largest = moqt::Location {
            group_id: 10,
            object_id: 5,
        };
        let key = setup_upstream(&table, TrackKey::new("ns", "track"));
        table.register_downstream_subscription(2, 100, key, Some(largest));
        let (track_key, loc) = Fetch
            .resolve_joined_subscription(2, 100, &table)
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

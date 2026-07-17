use std::sync::Arc;

use moqt::wire::FetchParams;
use tracing::Span;

use crate::modules::{
    control_message_forwarder::ControlMessageForwarder,
    core::handler::fetch::FetchHandler,
    enums::FetchErrorCode,
    relay::{
        cache::{
            store::TrackCacheStore,
            track_cache::{FetchRangeResolution, TrackCache},
        },
        egress::coordinator::{EgressCommand, EgressFetchRequest},
        ingress::fetch_ingest::{FetchIngest, FetchIngestStart},
    },
    sequences::tables::table::LocalPubSubDirectory,
    types::{SessionId, TrackKey},
    upstream_publisher_resolver::UpstreamPublisherResolver,
};

pub(crate) struct Fetch;

struct CacheTarget {
    cache: Arc<TrackCache>,
    start_location: moqt::Location,
    end_location: moqt::Location,
}

struct FetchTarget {
    track_key: TrackKey,
    track_namespace: String,
    track_name: String,
    start_location: moqt::Location,
    end_location: moqt::Location,
}

struct JoinedSubscriptionTarget {
    track_key: TrackKey,
    track_namespace: String,
    track_name: String,
    largest_location: moqt::Location,
}

struct LocationRange {
    start_location: moqt::Location,
    end_location: moqt::Location,
}

struct UpstreamFetch {
    track_namespace: String,
    track_name: String,
    start_location: moqt::Location,
    end_location: moqt::Location,
}

struct PreparedUpstreamFetch {
    handle: moqt::FetchHandle,
    upstream_publisher_session_id: SessionId,
}

/// Where the data for a FETCH will come from: the local cache, or an
/// upstream fetch for the range the cache cannot serve.
enum FetchSource {
    Cache(CacheTarget),
    Upstream(LocationRange),
}

/// A reason a FETCH could not be resolved, mapped to a FETCH_ERROR.
#[derive(Debug)]
enum FetchError {
    TrackNotFound,
    UnknownJoiningRequestId,
    NoObjectsPublished,
    InvalidRange,
    NoObjects,
}

impl FetchError {
    fn code(&self) -> FetchErrorCode {
        match self {
            Self::TrackNotFound => FetchErrorCode::TrackDoesNotExist,
            Self::UnknownJoiningRequestId => FetchErrorCode::InvalidJoiningRequestId,
            Self::NoObjectsPublished => FetchErrorCode::InvalidRange,
            Self::InvalidRange => FetchErrorCode::InvalidRange,
            Self::NoObjects => FetchErrorCode::NoObjects,
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            Self::TrackNotFound => "Track not found",
            Self::UnknownJoiningRequestId => "Unknown joining request id",
            Self::NoObjectsPublished => "No objects published",
            Self::InvalidRange => "Invalid fetch range",
            Self::NoObjects => "No objects in fetch range",
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
        let fetch_params = handler.fetch_params();
        let request_id = handler.request_id();

        let target = match self
            .resolve_fetch_target(session_id, fetch_params, table)
            .await
        {
            Ok(target) => target,
            Err(err) => {
                let _ = handler
                    .error(err.code() as u64, err.reason().to_string())
                    .await;
                return;
            }
        };

        let source = match self.resolve_fetch_source(&target, cache_store).await {
            Ok(source) => source,
            Err(err) => {
                let _ = handler
                    .error(err.code() as u64, err.reason().to_string())
                    .await;
                return;
            }
        };

        match source {
            FetchSource::Cache(CacheTarget {
                cache,
                start_location,
                end_location,
            }) => {
                // The relay does not yet track final End Of Track via PUBLISH_DONE,
                // so local cache responses cannot assert it.
                if let Err(e) = handler.ok(false, end_location).await {
                    tracing::error!(?e, "Failed to send FETCH_OK");
                    return;
                }

                // Delegate data delivery to egress.
                let fetch_request = EgressFetchRequest {
                    subscriber_session_id: session_id,
                    request_id,
                    cache,
                    start_location,
                    end_location,
                    group_order: handler.group_order(),
                };
                if let Err(e) = egress_sender
                    .send(EgressCommand::StartFetch(fetch_request))
                    .await
                {
                    tracing::error!(?e, "Failed to send fetch request to egress");
                }
            }
            FetchSource::Upstream(missing_range) => {
                // Joining Fetches forward as Standalone: the target is already
                // resolved to the absolute range whose end is the equivalent
                // Standalone Fetch encoding (§9.16.2.1, largest + 1).
                let start_location = missing_range.start_location;
                let request = Self::build_upstream_fetch(&target, missing_range);
                // This awaits the upstream FETCH_OK before returning to the session
                // worker; object ingestion continues in a background task afterwards.
                let Some(prepared) = self
                    .create_upstream_fetch(
                        table,
                        forwarder,
                        upstream_publisher_resolver,
                        handler.as_ref(),
                        request,
                    )
                    .await
                else {
                    return;
                };

                if let Err(e) = handler
                    .ok(prepared.handle.end_of_track, prepared.handle.end_location)
                    .await
                {
                    tracing::error!(?e, "Failed to send FETCH_OK to downstream");
                    return;
                }

                let cache = cache_store.get_or_create(&target.track_key);
                let egress_start = EgressFetchRequest {
                    subscriber_session_id: session_id,
                    request_id,
                    cache: cache.clone(),
                    start_location,
                    end_location: prepared.handle.end_location,
                    group_order: handler.group_order(),
                };
                let _fetch_ingest = FetchIngest::run(
                    forwarder.repository.clone(),
                    egress_sender.clone(),
                    FetchIngestStart {
                        upstream_publisher_session_id: prepared.upstream_publisher_session_id,
                        downstream_subscriber_session_id: session_id,
                        request_id,
                        fetch_handle: prepared.handle,
                        cache,
                        requested_start: start_location,
                        requested_end: egress_start.end_location,
                        egress_start,
                    },
                );
            }
        }
    }

    fn build_upstream_fetch(target: &FetchTarget, missing_range: LocationRange) -> UpstreamFetch {
        UpstreamFetch {
            track_namespace: target.track_namespace.clone(),
            track_name: target.track_name.clone(),
            start_location: missing_range.start_location,
            end_location: missing_range.end_location,
        }
    }

    /// Create upstream FETCH relay state before FETCH_OK is sent downstream.
    async fn create_upstream_fetch(
        &self,
        table: &dyn LocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
        handler: &dyn FetchHandler,
        request: UpstreamFetch,
    ) -> Option<PreparedUpstreamFetch> {
        let fetch_option = moqt::FetchOption {
            // Subscriber Priority is a mandatory FETCH field; forward the default
            // for now. Propagating the downstream request's priority belongs to
            // the priority-control work, tracked separately.
            subscriber_priority: moqt::FetchOption::default().subscriber_priority,
            group_order: handler.group_order(),
        };

        // Resolve upstream publisher via route registry / inter-relay.
        let upstream_key = match upstream_publisher_resolver
            .resolve(table, &request.track_namespace, &request.track_name)
            .await
        {
            Ok(Some(key)) => key,
            Ok(None) => {
                tracing::warn!(
                    track_namespace = %request.track_namespace,
                    track_name = %request.track_name,
                    "No upstream publisher found for fetch"
                );
                let _ = handler
                    .error(
                        FetchErrorCode::TrackDoesNotExist as u64,
                        FetchError::TrackNotFound.reason().to_string(),
                    )
                    .await;
                return None;
            }
            Err(err) => {
                // Display with alternate ({:#}) keeps the error chain but not the
                // backtrace: these are expected request-scoped failures.
                tracing::warn!(
                    err = %format!("{err:#}"),
                    track_namespace = %request.track_namespace,
                    track_name = %request.track_name,
                    "Failed to resolve upstream publisher for fetch"
                );
                let _ = handler
                    .error(
                        FetchErrorCode::InternalError as u64,
                        "Internal relay error".to_string(),
                    )
                    .await;
                return None;
            }
        };

        // Forward FETCH to the upstream publisher session.
        let handle = match forwarder
            .fetch(
                upstream_key.publisher_session_id,
                upstream_key.track_namespace.clone(),
                upstream_key.track_name.clone(),
                request.start_location,
                request.end_location,
                fetch_option,
            )
            .await
        {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    err = %format!("{err:#}"),
                    pub_session_id = upstream_key.publisher_session_id,
                    track_namespace = %request.track_namespace,
                    track_name = %request.track_name,
                    "Upstream FETCH failed"
                );
                let (error_code, reason) = Self::upstream_fetch_error_response(&err);
                let _ = handler.error(error_code, reason).await;
                return None;
            }
        };

        tracing::info!(
            pub_session_id = upstream_key.publisher_session_id,
            track_namespace = %request.track_namespace,
            track_name = %request.track_name,
            upstream_request_id = handle.request_id,
            "Upstream FETCH_OK received; starting cache fill"
        );

        Some(PreparedUpstreamFetch {
            handle,
            upstream_publisher_session_id: upstream_key.publisher_session_id,
        })
    }

    fn upstream_fetch_error_response(error: &anyhow::Error) -> (u64, String) {
        if let Some(fetch_error) = error.downcast_ref::<moqt::wire::RequestError>() {
            return (fetch_error.error_code, fetch_error.reason_phrase.clone());
        }
        if error.downcast_ref::<moqt::RequestTimeoutError>().is_some() {
            return (
                FetchErrorCode::Timeout as u64,
                "Upstream fetch timed out".to_string(),
            );
        }
        (
            FetchErrorCode::InternalError as u64,
            "Internal relay error".to_string(),
        )
    }

    async fn resolve_fetch_target(
        &self,
        session_id: SessionId,
        fetch_params: FetchParams,
        table: &dyn LocalPubSubDirectory,
    ) -> Result<FetchTarget, FetchError> {
        match fetch_params {
            FetchParams::Standalone {
                track_namespace,
                track_name,
                start_location,
                end_location,
            } => {
                let track_namespace = track_namespace.join("/");
                Ok(FetchTarget {
                    track_key: TrackKey::new(&track_namespace, &track_name),
                    track_namespace,
                    track_name,
                    start_location,
                    end_location,
                })
            }
            FetchParams::RelativeJoining {
                joining_request_id,
                joining_start,
            } => {
                self.resolve_relative_joining_target(
                    session_id,
                    joining_request_id,
                    joining_start,
                    table,
                )
                .await
            }
            FetchParams::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => {
                self.resolve_absolute_joining_target(
                    session_id,
                    joining_request_id,
                    joining_start,
                    table,
                )
                .await
            }
        }
    }

    async fn resolve_relative_joining_target(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        joining_start: u64,
        table: &dyn LocalPubSubDirectory,
    ) -> Result<FetchTarget, FetchError> {
        let joined_target = self
            .resolve_joined_subscription(session_id, joining_request_id, table)
            .await?;
        let largest_location = joined_target.largest_location;
        Ok(FetchTarget {
            track_key: joined_target.track_key,
            track_namespace: joined_target.track_namespace,
            track_name: joined_target.track_name,
            start_location: moqt::Location {
                group_id: largest_location.group_id.saturating_sub(joining_start),
                object_id: 0,
            },
            end_location: Self::location_after_largest(largest_location),
        })
    }

    async fn resolve_absolute_joining_target(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        joining_start: u64,
        table: &dyn LocalPubSubDirectory,
    ) -> Result<FetchTarget, FetchError> {
        let joined_target = self
            .resolve_joined_subscription(session_id, joining_request_id, table)
            .await?;
        let largest_location = joined_target.largest_location;
        let start_location = moqt::Location {
            group_id: joining_start,
            object_id: 0,
        };
        if Self::location_is_after_largest(start_location, largest_location) {
            return Err(FetchError::InvalidRange);
        }
        Ok(FetchTarget {
            track_key: joined_target.track_key,
            track_namespace: joined_target.track_namespace,
            track_name: joined_target.track_name,
            start_location,
            end_location: Self::location_after_largest(largest_location),
        })
    }

    async fn resolve_fetch_source(
        &self,
        target: &FetchTarget,
        cache_store: &TrackCacheStore,
    ) -> Result<FetchSource, FetchError> {
        let start_location = target.start_location;
        let end_location = target.end_location;
        let Some(cache) = cache_store.get(&target.track_key) else {
            tracing::debug!(
                track_namespace = %target.track_namespace,
                track_name = %target.track_name,
                "No cached track for fetch"
            );
            return Ok(FetchSource::Upstream(LocationRange {
                start_location,
                end_location,
            }));
        };
        let source = match cache
            .resolve_fetch_range(start_location, end_location)
            .await
        {
            FetchRangeResolution::Serve { end_location } => FetchSource::Cache(CacheTarget {
                cache,
                start_location,
                end_location,
            }),
            FetchRangeResolution::InvalidRange => return Err(FetchError::InvalidRange),
            FetchRangeResolution::NoObjects => return Err(FetchError::NoObjects),
            FetchRangeResolution::NotCovered => {
                tracing::debug!(
                    track_namespace = %target.track_namespace,
                    track_name = %target.track_name,
                    start_group_id = start_location.group_id,
                    start_object_id = start_location.object_id,
                    end_group_id = end_location.group_id,
                    end_object_id = end_location.object_id,
                    "Cached track cannot serve full fetch range locally"
                );
                FetchSource::Upstream(LocationRange {
                    start_location,
                    end_location,
                })
            }
        };
        Ok(source)
    }

    /// Resolves the joined subscription's track and its Largest Location at subscribe time,
    /// shared by Relative and Absolute Joining Fetch.
    ///
    /// When no objects existed at subscribe time (`start_location` is `None`), §9.16.2
    /// requires rejecting the Joining Fetch with INVALID_RANGE.
    async fn resolve_joined_subscription(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        table: &dyn LocalPubSubDirectory,
    ) -> Result<JoinedSubscriptionTarget, FetchError> {
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

        Ok(JoinedSubscriptionTarget {
            track_key: active_upstream.track_key,
            track_namespace: downstream_sub.upstream_key.track_namespace,
            track_name: downstream_sub.upstream_key.track_name,
            largest_location: largest,
        })
    }

    fn location_after_largest(largest: moqt::Location) -> moqt::Location {
        moqt::Location {
            group_id: largest.group_id,
            object_id: largest.object_id + 1,
        }
    }

    fn location_is_after_largest(location: moqt::Location, largest: moqt::Location) -> bool {
        location.group_id > largest.group_id
            || (location.group_id == largest.group_id && location.object_id > largest.object_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        core::data_object::DataObject,
        enums::ContentExists,
        relay::types::StreamSubgroupId,
        sequences::tables::{
            hashmap_table::InMemoryLocalPubSubDirectory,
            table::{
                ActiveUpstreamSubscription, UpstreamSubscriptionKey, UpstreamSubscriptionOrigin,
            },
        },
    };
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, SubgroupHeader, SubgroupId, SubgroupObject, SubgroupObjectField};

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

    fn make_header(group_id: u64) -> DataObject {
        DataObject::SubgroupHeader(SubgroupHeader::new(
            0,
            group_id,
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
            subgroup_object: SubgroupObject::new_payload(Bytes::new()),
        })
    }

    async fn fill_group_with_ids(
        cache_store: &TrackCacheStore,
        track_key: &TrackKey,
        group_id: u64,
        object_ids: &[u64],
    ) {
        let subgroup = StreamSubgroupId::Value(0);
        let cache = cache_store.get_or_create(track_key);
        cache
            .append_live_stream_object(group_id, &subgroup, None, make_header(group_id))
            .await;
        for &object_id in object_ids {
            cache
                .append_live_stream_object(group_id, &subgroup, Some(object_id), make_object())
                .await;
        }
    }

    fn standalone_fetch_params(
        start_location: moqt::Location,
        end_location: moqt::Location,
    ) -> FetchParams {
        FetchParams::Standalone {
            track_namespace: vec!["ns".to_string()],
            track_name: "track".to_string(),
            start_location,
            end_location,
        }
    }

    #[test]
    fn upstream_timeout_maps_to_fetch_error_timeout() {
        let error = anyhow::Error::new(moqt::RequestTimeoutError);
        let (code, _reason) = Fetch::upstream_fetch_error_response(&error);
        assert_eq!(code, FetchErrorCode::Timeout as u64);
    }

    #[test]
    fn upstream_request_error_code_is_relayed_verbatim() {
        let error = anyhow::Error::new(moqt::wire::RequestError {
            request_id: 7,
            error_code: 0x3,
            reason_phrase: "not supported".to_string(),
        });
        let (code, reason) = Fetch::upstream_fetch_error_response(&error);
        assert_eq!(code, 0x3);
        assert_eq!(reason, "not supported");
    }

    async fn resolve_target_and_source(
        session_id: SessionId,
        fetch_params: FetchParams,
        table: &dyn LocalPubSubDirectory,
        cache_store: &Arc<TrackCacheStore>,
    ) -> Result<(FetchTarget, FetchSource), FetchError> {
        let target = Fetch
            .resolve_fetch_target(session_id, fetch_params, table)
            .await?;
        let source = Fetch.resolve_fetch_source(&target, cache_store).await?;
        Ok((target, source))
    }

    #[tokio::test]
    async fn fetch_source_is_upstream_when_track_entry_missing() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::Location {
            group_id: 0,
            object_id: 1,
        };
        let (target, source) = resolve_target_and_source(
            2,
            standalone_fetch_params(start, end),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await
        .unwrap();
        match source {
            FetchSource::Upstream(missing_range) => {
                assert_eq!(target.track_namespace, "ns");
                assert_eq!(target.track_name, "track");
                assert_eq!(missing_range.start_location, start);
                assert_eq!(missing_range.end_location, end);
            }
            FetchSource::Cache(_) => panic!("expected upstream fetch"),
        }
    }

    #[tokio::test]
    async fn fetch_source_is_upstream_when_cache_entry_is_empty() {
        let cache_store = Arc::new(TrackCacheStore::new());
        cache_store.get_or_create(&TrackKey::new("ns", "track"));
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::Location {
            group_id: 0,
            object_id: 1,
        };
        let (_, source) = resolve_target_and_source(
            2,
            standalone_fetch_params(start, end),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await
        .unwrap();
        assert!(matches!(source, FetchSource::Upstream(_)));
    }

    #[tokio::test]
    async fn fetch_source_is_cache_for_known_gapped_object_ids() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        fill_group_with_ids(&cache_store, &track_key, 0, &[0, 2]).await;
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::Location {
            group_id: 0,
            object_id: 3,
        };
        let (_, source) = resolve_target_and_source(
            2,
            standalone_fetch_params(start, end),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await
        .unwrap();
        assert!(matches!(source, FetchSource::Cache(_)));
    }

    #[tokio::test]
    async fn fetch_source_is_upstream_when_request_starts_before_cache_coverage() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        fill_group_with_ids(&cache_store, &track_key, 1, &[0, 1]).await;
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::Location {
            group_id: 1,
            object_id: 2,
        };
        let (_, source) = resolve_target_and_source(
            2,
            standalone_fetch_params(start, end),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await
        .unwrap();
        match source {
            FetchSource::Upstream(missing_range) => {
                assert_eq!(missing_range.start_location, start);
                assert_eq!(missing_range.end_location, end);
            }
            FetchSource::Cache(_) => {
                panic!("leading cache gaps must be forwarded upstream")
            }
        }
    }

    #[tokio::test]
    async fn fetch_source_clamps_standalone_end_when_request_exceeds_largest() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        fill_group_with_ids(&cache_store, &track_key, 0, &[0, 1, 2]).await;
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let requested_end = moqt::Location {
            group_id: 2,
            object_id: 0,
        };
        let (_, source) = resolve_target_and_source(
            2,
            standalone_fetch_params(start, requested_end),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await
        .unwrap();
        match source {
            FetchSource::Cache(resolved) => {
                assert_eq!(
                    resolved.end_location,
                    moqt::Location {
                        group_id: 0,
                        object_id: 3
                    }
                );
            }
            FetchSource::Upstream(_) => {
                panic!("covered fetch range should be served locally with clamped end")
            }
        }
    }

    #[tokio::test]
    async fn fetch_source_rejects_standalone_start_after_largest_as_invalid_range() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        fill_group_with_ids(&cache_store, &track_key, 0, &[0]).await;
        cache_store.get_or_create(&track_key).begin_live_ingest();
        let result = resolve_target_and_source(
            2,
            standalone_fetch_params(
                moqt::Location {
                    group_id: 0,
                    object_id: 1,
                },
                moqt::Location {
                    group_id: 0,
                    object_id: 2,
                },
            ),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await;
        assert!(matches!(result, Err(FetchError::InvalidRange)));
    }

    #[tokio::test]
    async fn fetch_source_rejects_standalone_covered_empty_range_as_no_objects() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        fill_group_with_ids(&cache_store, &track_key, 0, &[0]).await;
        let subgroup = StreamSubgroupId::Value(0);
        let cache = cache_store.get_or_create(&track_key);
        cache
            .append_stream_object(1, &subgroup, None, make_header(1))
            .await;
        cache.close_stream_subgroup(1, &subgroup).await;
        fill_group_with_ids(&cache_store, &track_key, 2, &[0]).await;

        let result = resolve_target_and_source(
            2,
            standalone_fetch_params(
                moqt::Location {
                    group_id: 1,
                    object_id: 0,
                },
                moqt::Location {
                    group_id: 1,
                    object_id: 0,
                },
            ),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await;
        assert!(matches!(result, Err(FetchError::NoObjects)));
    }

    #[tokio::test]
    async fn fetch_source_is_cache_when_standalone_cache_covers_range() {
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        fill_group_with_ids(&cache_store, &track_key, 1, &[2, 3]).await;
        let start = moqt::Location {
            group_id: 1,
            object_id: 2,
        };
        let end = moqt::Location {
            group_id: 1,
            object_id: 4,
        };
        let (_, source) = resolve_target_and_source(
            2,
            standalone_fetch_params(start, end),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        )
        .await
        .unwrap();
        match source {
            FetchSource::Cache(resolved) => {
                assert_eq!(resolved.start_location, start);
                assert_eq!(resolved.end_location, end);
            }
            FetchSource::Upstream(_) => {
                panic!("expected cache fetch readiness")
            }
        }
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
        // No objects in cache either, so NoObjectsPublished.
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
        let context = Fetch
            .resolve_joined_subscription(2, 100, &table)
            .await
            .unwrap();
        assert_eq!(context.track_key, TrackKey::new("ns", "track"));
        assert_eq!(context.track_namespace, "ns");
        assert_eq!(context.track_name, "track");
        assert_eq!(
            context.largest_location,
            moqt::Location {
                group_id: 10,
                object_id: 5
            }
        );
    }

    #[tokio::test]
    async fn fetch_source_is_upstream_when_relative_joining_cache_does_not_cover_range() {
        let table = InMemoryLocalPubSubDirectory::new();
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let upstream_key = setup_upstream(&table, track_key.clone());
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 1,
                object_id: 1,
            }),
        );
        fill_group_with_ids(&cache_store, &track_key, 1, &[0, 1]).await;

        let (target, source) = resolve_target_and_source(
            2,
            FetchParams::RelativeJoining {
                joining_request_id: 100,
                joining_start: 1,
            },
            &table,
            &cache_store,
        )
        .await
        .unwrap();

        match source {
            FetchSource::Upstream(missing_range) => {
                assert_eq!(target.track_namespace, "ns");
                assert_eq!(target.track_name, "track");
                assert_eq!(
                    missing_range.start_location,
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    }
                );
                assert_eq!(
                    missing_range.end_location,
                    moqt::Location {
                        group_id: 1,
                        object_id: 2
                    }
                );
            }
            FetchSource::Cache(_) => {
                panic!("joining fetch with missing local coverage must be forwarded upstream")
            }
        }
    }

    #[tokio::test]
    async fn absolute_joining_forwards_resolved_range_upstream() {
        let table = InMemoryLocalPubSubDirectory::new();
        let cache_store = Arc::new(TrackCacheStore::new());
        let upstream_key = setup_upstream(&table, TrackKey::new("ns", "track"));
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 2,
                object_id: 3,
            }),
        );

        let (_, source) = resolve_target_and_source(
            2,
            FetchParams::AbsoluteJoining {
                joining_request_id: 100,
                joining_start: 1,
            },
            &table,
            &cache_store,
        )
        .await
        .unwrap();

        // Upstream range: start = {joining_start, 0}, end = largest + 1
        // (the equivalent Standalone Fetch encoding, §9.16.2.1).
        match source {
            FetchSource::Upstream(missing_range) => {
                assert_eq!(
                    missing_range.start_location,
                    moqt::Location {
                        group_id: 1,
                        object_id: 0
                    }
                );
                assert_eq!(
                    missing_range.end_location,
                    moqt::Location {
                        group_id: 2,
                        object_id: 4
                    }
                );
            }
            FetchSource::Cache(_) => panic!("cold cache must forward upstream"),
        }
    }

    #[tokio::test]
    async fn relative_joining_start_saturates_at_group_zero() {
        let table = InMemoryLocalPubSubDirectory::new();
        let cache_store = Arc::new(TrackCacheStore::new());
        let upstream_key = setup_upstream(&table, TrackKey::new("ns", "track"));
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 1,
                object_id: 1,
            }),
        );

        let (_, source) = resolve_target_and_source(
            2,
            FetchParams::RelativeJoining {
                joining_request_id: 100,
                // Further back than the track head: the start group saturates to 0.
                joining_start: 5,
            },
            &table,
            &cache_store,
        )
        .await
        .unwrap();

        match source {
            FetchSource::Upstream(missing_range) => {
                assert_eq!(
                    missing_range.start_location,
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    }
                );
                assert_eq!(
                    missing_range.end_location,
                    moqt::Location {
                        group_id: 1,
                        object_id: 2
                    }
                );
            }
            FetchSource::Cache(_) => panic!("cold cache must forward upstream"),
        }
    }

    #[tokio::test]
    async fn fetch_source_rejects_absolute_joining_start_after_largest_without_cache() {
        let table = InMemoryLocalPubSubDirectory::new();
        let cache_store = Arc::new(TrackCacheStore::new());
        let upstream_key = setup_upstream(&table, TrackKey::new("ns", "track"));
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 1,
                object_id: 1,
            }),
        );

        let result = resolve_target_and_source(
            2,
            FetchParams::AbsoluteJoining {
                joining_request_id: 100,
                joining_start: 2,
            },
            &table,
            &cache_store,
        )
        .await;

        assert!(matches!(result, Err(FetchError::InvalidRange)));
    }
}

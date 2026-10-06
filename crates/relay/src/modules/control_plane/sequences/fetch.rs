mod upstream_fetch_task;

use upstream_fetch_task::{UpstreamFetchStart, UpstreamFetchTask};

use std::sync::Arc;

use moqt::wire::FetchParams;
use tracing::Span;

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::{
        cache::{
            store::TrackCacheStore,
            track_cache::{FetchRangeResolution, TrackCache},
        },
        egress::coordinator::{EgressCommand, EgressFetchRequest},
    },
    domain::{
        error_code::FetchErrorCode, pub_sub_directory::InMemoryLocalPubSubDirectory,
        session_id::SessionId, track_key::TrackKey,
    },
    session::{handler::fetch::FetchHandler, session_event::SessionEvent},
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

struct PreparedUpstreamFetch {
    handle: moqt::FetchHandle,
    upstream_publisher_session_id: SessionId,
}

enum FetchSource {
    Cache(CacheTarget),
    Upstream,
}

#[derive(Debug)]
enum FetchError {
    TrackNotFound,
    UnknownJoiningRequestId,
    NoObjectsPublished,
    InvalidRange,
    NoObjects,
    MalformedTrack,
}

impl FetchError {
    fn code(&self) -> FetchErrorCode {
        match self {
            Self::TrackNotFound => FetchErrorCode::TrackDoesNotExist,
            Self::UnknownJoiningRequestId => FetchErrorCode::InvalidJoiningRequestId,
            Self::NoObjectsPublished => FetchErrorCode::InvalidRange,
            Self::InvalidRange => FetchErrorCode::InvalidRange,
            Self::NoObjects => FetchErrorCode::NoObjects,
            Self::MalformedTrack => FetchErrorCode::MalformedTrack,
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            Self::TrackNotFound => "Track not found",
            Self::UnknownJoiningRequestId => "Unknown joining request id",
            Self::NoObjectsPublished => "No objects published",
            Self::InvalidRange => "Invalid fetch range",
            Self::NoObjects => "No objects in fetch range",
            Self::MalformedTrack => "Malformed track",
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
        table: &Arc<InMemoryLocalPubSubDirectory>,
        cache_store: &Arc<TrackCacheStore>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        session_event_sender: &tokio::sync::mpsc::UnboundedSender<SessionEvent>,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
        handler: Box<dyn FetchHandler>,
    ) {
        let fetch_params = handler.fetch_params();
        let request_id = handler.request_id();

        let (target, source) =
            match self.resolve_target_and_source(session_id, fetch_params, table, cache_store) {
                Ok(resolved) => resolved,
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
            FetchSource::Upstream => {
                // Joining Fetches forward as Standalone: the target is already
                // resolved to the absolute range whose end is the equivalent
                // Standalone Fetch encoding (§9.16.2.1, largest + 1).
                let _upstream_fetch = UpstreamFetchTask::run(UpstreamFetchStart {
                    session_id,
                    handler,
                    target,
                    table: table.clone(),
                    forwarder: forwarder.clone(),
                    upstream_publisher_resolver: upstream_publisher_resolver.clone(),
                    cache_store: cache_store.clone(),
                    egress_sender: egress_sender.clone(),
                    session_event_sender: session_event_sender.clone(),
                });
            }
        }
    }

    async fn create_upstream_fetch(
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
        handler: &dyn FetchHandler,
        target: &FetchTarget,
    ) -> Option<PreparedUpstreamFetch> {
        let fetch_option = moqt::FetchOption {
            subscriber_priority: moqt::FetchOption::default().subscriber_priority,
            group_order: handler.group_order(),
        };

        let upstream_key = match upstream_publisher_resolver
            .resolve(table, &target.track_namespace, &target.track_name)
            .await
            .map(|publishers| publishers.into_iter().next())
        {
            Ok(Some(key)) => key,
            Ok(None) => {
                tracing::warn!(
                    track_namespace = %target.track_namespace,
                    track_name = %target.track_name,
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
                    track_namespace = %target.track_namespace,
                    track_name = %target.track_name,
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

        let handle = match forwarder
            .fetch(
                upstream_key.publisher_session_id,
                upstream_key.track_namespace.clone(),
                upstream_key.track_name.clone(),
                target.start_location,
                target.end_location,
                fetch_option,
            )
            .await
        {
            Ok(pair) => pair,
            Err(err) => {
                tracing::warn!(
                    err = %format!("{err:#}"),
                    pub_session_id = upstream_key.publisher_session_id,
                    track_namespace = %target.track_namespace,
                    track_name = %target.track_name,
                    "Upstream FETCH failed"
                );
                let (error_code, reason) = Self::upstream_fetch_error_response(&err);
                let _ = handler.error(error_code, reason).await;
                return None;
            }
        };

        tracing::info!(
            pub_session_id = upstream_key.publisher_session_id,
            track_namespace = %target.track_namespace,
            track_name = %target.track_name,
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

    fn resolve_target_and_source(
        &self,
        session_id: SessionId,
        fetch_params: FetchParams,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
    ) -> Result<(FetchTarget, FetchSource), FetchError> {
        let target = self.resolve_fetch_target(session_id, fetch_params, table)?;
        if cache_store
            .get(&target.track_key)
            .is_some_and(|cache| cache.is_malformed())
        {
            return Err(FetchError::MalformedTrack);
        }
        let source = self.resolve_fetch_source(&target, cache_store)?;
        Ok((target, source))
    }

    fn resolve_fetch_target(
        &self,
        session_id: SessionId,
        fetch_params: FetchParams,
        table: &InMemoryLocalPubSubDirectory,
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
            } => self.resolve_joining_target(session_id, joining_request_id, table, |largest| {
                largest.group_id.saturating_sub(joining_start)
            }),
            FetchParams::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => self
                .resolve_joining_target(session_id, joining_request_id, table, |_| joining_start),
        }
    }

    fn resolve_fetch_source(
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
            return Ok(FetchSource::Upstream);
        };
        let source = match cache.resolve_fetch_range(start_location, end_location) {
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
                FetchSource::Upstream
            }
        };
        Ok(source)
    }

    /// When no objects existed at subscribe time (`start_location` is `None`), §9.16.2
    /// requires rejecting the Joining Fetch with INVALID_RANGE.
    fn resolve_joining_target(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        table: &InMemoryLocalPubSubDirectory,
        start_group: impl FnOnce(moqt::Location) -> u64,
    ) -> Result<FetchTarget, FetchError> {
        let Some(downstream_sub) =
            table.get_downstream_subscription(session_id, joining_request_id)
        else {
            tracing::warn!(
                joining_request_id,
                "Joining fetch references unknown subscription"
            );
            return Err(FetchError::UnknownJoiningRequestId);
        };

        if table
            .get_upstream_track(&downstream_sub.track_key)
            .is_none()
        {
            tracing::warn!("Joined subscription has no active upstream subscription");
            return Err(FetchError::TrackNotFound);
        }

        let Some(largest) = downstream_sub.start_location else {
            tracing::warn!("Joining fetch: no objects published at subscribe time");
            return Err(FetchError::NoObjectsPublished);
        };

        let start_location = moqt::Location {
            group_id: start_group(largest),
            object_id: 0,
        };
        if start_location > largest {
            return Err(FetchError::InvalidRange);
        }
        Ok(FetchTarget {
            track_namespace: downstream_sub.track_key.track_namespace.clone(),
            track_name: downstream_sub.track_key.track_name.clone(),
            track_key: downstream_sub.track_key,
            start_location,
            end_location: Self::location_after_largest(largest),
        })
    }

    fn location_after_largest(largest: moqt::Location) -> moqt::Location {
        moqt::Location {
            group_id: largest.group_id,
            object_id: largest.object_id + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::pub_sub_directory::entry::UpstreamSubscriptionOrigin,
        test_support::{
            directory_fixtures::table_with_upstream,
            relay_harness::fixtures::cached_object::{insert_closed_group, open_group},
        },
    };

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
        // Arrange
        let error = anyhow::Error::new(moqt::RequestTimeoutError);

        // Act
        let (code, _reason) = Fetch::upstream_fetch_error_response(&error);

        // Assert
        assert_eq!(code, FetchErrorCode::Timeout as u64);
    }

    #[test]
    fn upstream_request_error_code_is_relayed_verbatim() {
        // Arrange
        let error = anyhow::Error::new(moqt::wire::RequestError {
            request_id: 7,
            error_code: 0x3,
            reason_phrase: "not supported".to_string(),
        });

        // Act
        let (code, reason) = Fetch::upstream_fetch_error_response(&error);

        // Assert
        assert_eq!(code, 0x3);
        assert_eq!(reason, "not supported");
    }

    #[test]
    fn fetch_source_is_upstream_when_track_entry_missing() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::Location {
            group_id: 0,
            object_id: 1,
        };

        // Act
        let (target, source) = Fetch
            .resolve_target_and_source(
                2,
                standalone_fetch_params(start, end),
                &InMemoryLocalPubSubDirectory::new(),
                &cache_store,
            )
            .unwrap();

        // Assert
        match source {
            FetchSource::Upstream => {
                assert_eq!(target.track_namespace, "ns");
                assert_eq!(target.track_name, "track");
                assert_eq!(target.start_location, start);
                assert_eq!(target.end_location, end);
            }
            FetchSource::Cache(_) => panic!("expected upstream fetch"),
        }
    }

    #[test]
    fn fetch_source_is_upstream_when_cache_entry_is_empty() {
        // Arrange
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

        // Act
        let (_, source) = Fetch
            .resolve_target_and_source(
                2,
                standalone_fetch_params(start, end),
                &InMemoryLocalPubSubDirectory::new(),
                &cache_store,
            )
            .unwrap();

        // Assert
        assert!(matches!(source, FetchSource::Upstream));
    }

    #[test]
    fn fetch_source_is_upstream_across_a_skipped_object_id() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let _open_g0 = open_group(&cache, 0, &[0, 2]);
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::Location {
            group_id: 0,
            object_id: 3,
        };

        // Act
        let (_, source) = Fetch
            .resolve_target_and_source(
                2,
                standalone_fetch_params(start, end),
                &InMemoryLocalPubSubDirectory::new(),
                &cache_store,
            )
            .unwrap();

        // Assert
        assert!(matches!(source, FetchSource::Upstream));
    }

    #[test]
    fn fetch_source_is_upstream_when_request_starts_before_cache_coverage() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let _open_g1 = open_group(&cache, 1, &[0, 1]);
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::Location {
            group_id: 1,
            object_id: 2,
        };

        // Act
        let (target, source) = Fetch
            .resolve_target_and_source(
                2,
                standalone_fetch_params(start, end),
                &InMemoryLocalPubSubDirectory::new(),
                &cache_store,
            )
            .unwrap();

        // Assert
        match source {
            FetchSource::Upstream => {
                assert_eq!(target.start_location, start);
                assert_eq!(target.end_location, end);
            }
            FetchSource::Cache(_) => {
                panic!("leading cache gaps must be forwarded upstream")
            }
        }
    }

    #[test]
    fn fetch_source_clamps_standalone_end_when_request_exceeds_largest() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let _open_g0 = open_group(&cache, 0, &[0, 1, 2]);
        let start = moqt::Location {
            group_id: 0,
            object_id: 0,
        };
        let requested_end = moqt::Location {
            group_id: 2,
            object_id: 0,
        };

        // Act
        let (_, source) = Fetch
            .resolve_target_and_source(
                2,
                standalone_fetch_params(start, requested_end),
                &InMemoryLocalPubSubDirectory::new(),
                &cache_store,
            )
            .unwrap();

        // Assert
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
            FetchSource::Upstream => {
                panic!("covered fetch range should be served locally with clamped end")
            }
        }
    }

    #[test]
    fn fetch_source_rejects_standalone_start_after_largest_as_invalid_range() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let _open_g0 = open_group(&cache, 0, &[0]);
        cache.begin_live_ingest();

        // Act
        let result = Fetch.resolve_target_and_source(
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
        );

        // Assert
        assert!(matches!(result, Err(FetchError::InvalidRange)));
    }

    #[test]
    fn fetch_source_rejects_standalone_covered_empty_range_as_no_objects() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let _open_g0 = open_group(&cache, 0, &[0]);
        insert_closed_group(&cache, 1, &[]);
        let _open_g2 = open_group(&cache, 2, &[0]);

        // Act
        let result = Fetch.resolve_target_and_source(
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
        );

        // Assert
        assert!(matches!(result, Err(FetchError::NoObjects)));
    }

    #[test]
    fn fetch_source_is_cache_when_standalone_cache_covers_range() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let _open_g1 = open_group(&cache, 1, &[2, 3]);
        let start = moqt::Location {
            group_id: 1,
            object_id: 2,
        };
        let end = moqt::Location {
            group_id: 1,
            object_id: 4,
        };

        // Act
        let (_, source) = Fetch
            .resolve_target_and_source(
                2,
                standalone_fetch_params(start, end),
                &InMemoryLocalPubSubDirectory::new(),
                &cache_store,
            )
            .unwrap();

        // Assert
        match source {
            FetchSource::Cache(resolved) => {
                assert_eq!(resolved.start_location, start);
                assert_eq!(resolved.end_location, end);
            }
            FetchSource::Upstream => {
                panic!("expected cache fetch readiness")
            }
        }
    }

    fn relative_joining_fetch_params(joining_request_id: u64) -> FetchParams {
        FetchParams::RelativeJoining {
            joining_request_id,
            joining_start: 0,
        }
    }

    #[test]
    fn resolve_joining_target_unknown_request_id() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();

        // Act
        let result = Fetch.resolve_fetch_target(1, relative_joining_fetch_params(999), &table);

        // Assert
        assert!(matches!(result, Err(FetchError::UnknownJoiningRequestId)));
    }

    #[test]
    fn resolve_joining_target_no_objects_published() {
        // Arrange
        let (table, key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(2, 100, key, None);

        // Act
        let result = Fetch.resolve_fetch_target(2, relative_joining_fetch_params(100), &table);

        // Assert
        assert!(matches!(result, Err(FetchError::NoObjectsPublished)));
    }

    #[test]
    fn resolve_joining_target_ends_after_stored_largest() {
        // Arrange
        let largest = moqt::Location {
            group_id: 10,
            object_id: 5,
        };
        let (table, key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(2, 100, key, Some(largest));

        // Act
        let target = Fetch
            .resolve_fetch_target(2, relative_joining_fetch_params(100), &table)
            .unwrap();

        // Assert
        assert_eq!(target.track_key, TrackKey::new("ns", "track"));
        assert_eq!(target.track_namespace, "ns");
        assert_eq!(target.track_name, "track");
        assert_eq!(
            target.end_location,
            moqt::Location {
                group_id: 10,
                object_id: 6
            }
        );
    }

    #[test]
    fn fetch_source_is_upstream_when_relative_joining_cache_does_not_cover_range() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let track_key = TrackKey::new("ns", "track");
        let cache = cache_store.get_or_create(&track_key);
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 1,
                object_id: 1,
            }),
        );
        let _open_g1 = open_group(&cache, 1, &[0, 1]);

        // Act
        let (target, source) = Fetch
            .resolve_target_and_source(
                2,
                FetchParams::RelativeJoining {
                    joining_request_id: 100,
                    joining_start: 1,
                },
                &table,
                &cache_store,
            )
            .unwrap();

        // Assert
        match source {
            FetchSource::Upstream => {
                assert_eq!(target.track_namespace, "ns");
                assert_eq!(target.track_name, "track");
                assert_eq!(
                    target.start_location,
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    }
                );
                assert_eq!(
                    target.end_location,
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

    #[test]
    fn absolute_joining_forwards_resolved_range_upstream() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 2,
                object_id: 3,
            }),
        );

        // Act
        let (target, source) = Fetch
            .resolve_target_and_source(
                2,
                FetchParams::AbsoluteJoining {
                    joining_request_id: 100,
                    joining_start: 1,
                },
                &table,
                &cache_store,
            )
            .unwrap();

        // Assert: start = {joining_start, 0}, end = largest + 1 (Standalone encoding, §9.16.2.1)
        match source {
            FetchSource::Upstream => {
                assert_eq!(
                    target.start_location,
                    moqt::Location {
                        group_id: 1,
                        object_id: 0
                    }
                );
                assert_eq!(
                    target.end_location,
                    moqt::Location {
                        group_id: 2,
                        object_id: 4
                    }
                );
            }
            FetchSource::Cache(_) => panic!("cold cache must forward upstream"),
        }
    }

    #[test]
    fn relative_joining_start_saturates_at_group_zero() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 1,
                object_id: 1,
            }),
        );

        // Act
        let (target, source) = Fetch
            .resolve_target_and_source(
                2,
                FetchParams::RelativeJoining {
                    joining_request_id: 100,
                    joining_start: 5,
                },
                &table,
                &cache_store,
            )
            .unwrap();

        // Assert
        match source {
            FetchSource::Upstream => {
                assert_eq!(
                    target.start_location,
                    moqt::Location {
                        group_id: 0,
                        object_id: 0
                    }
                );
                assert_eq!(
                    target.end_location,
                    moqt::Location {
                        group_id: 1,
                        object_id: 2
                    }
                );
            }
            FetchSource::Cache(_) => panic!("cold cache must forward upstream"),
        }
    }

    #[test]
    fn fetch_source_rejects_absolute_joining_start_after_largest_without_cache() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let (table, upstream_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(
            2,
            100,
            upstream_key,
            Some(moqt::Location {
                group_id: 1,
                object_id: 1,
            }),
        );

        // Act
        let result = Fetch.resolve_target_and_source(
            2,
            FetchParams::AbsoluteJoining {
                joining_request_id: 100,
                joining_start: 2,
            },
            &table,
            &cache_store,
        );

        // Assert
        assert!(matches!(result, Err(FetchError::InvalidRange)));
    }
}

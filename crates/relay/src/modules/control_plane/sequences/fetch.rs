mod upstream_fetch_task;

use moqt::RequestErrorCode;
use upstream_fetch_task::{UpstreamFetchStart, UpstreamFetchTask};

use std::{sync::Arc, time::Duration};

use moqt::{
    fetch::{FetchParams, JoinedSubscription, JoiningFetchRefusal},
    object::ContentExists,
};
use tracing::Span;

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::malformed_track::refused_as_malformed,
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::{
        cache::{
            store::TrackCacheStore,
            track_cache::{FetchRangeResolution, TrackCache},
        },
        egress::coordinator::{EgressCommand, EgressFetchRequest},
        ingress::fetch_ingest::FillBudget,
    },
    domain::{
        pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId,
        session_peer::SessionPeer, track_key::TrackKey,
    },
    session::{handler::fetch::FetchHandler, session_event::SessionEvent},
};

pub(crate) struct Fetch {
    pub(crate) fill_timeout: Duration,
}

struct CacheTarget {
    cache: Arc<TrackCache>,
    start_location: moqt::object::Location,
    end_location: moqt::object::Location,
}

struct FetchTarget {
    track_key: TrackKey,
    track_namespace: String,
    track_name: String,
    start_location: moqt::object::Location,
    end_location: moqt::object::Location,
}

struct PreparedUpstreamFetch {
    handle: moqt::fetch::FetchHandle,
    upstream_publisher_session_id: SessionId,
}

enum FetchSource {
    Cache(CacheTarget),
    Upstream,
}

/// Why no upstream FETCH was made; the downstream FETCH is already answered.
#[derive(Debug, PartialEq)]
enum NoUpstreamFetch {
    Refused,
    /// The publisher refused the track as malformed.
    MalformedTrack(SessionId),
}

#[derive(Debug)]
enum FetchError {
    TrackNotFound,
    UnknownJoiningRequestId,
    JoinedNotLargestObject,
    JoinedNotForwarding,
    /// A `JoiningFetchRefusal` variant this relay has no mapping for (the enum
    /// is non_exhaustive).
    JoiningRefused,
    NoObjectsPublished,
    InvalidRange,
    NoObjects,
    MalformedTrack,
}

impl FetchError {
    /// `None` when the FETCH is a protocol violation rather than a FETCH_ERROR.
    fn code(&self) -> Option<RequestErrorCode> {
        match self {
            Self::TrackNotFound => Some(RequestErrorCode::TrackDoesNotExist),
            Self::UnknownJoiningRequestId => Some(RequestErrorCode::InvalidJoiningRequestId),
            Self::JoinedNotLargestObject => None,
            Self::JoinedNotForwarding => Some(RequestErrorCode::InvalidRange),
            Self::JoiningRefused => Some(RequestErrorCode::InternalError),
            Self::NoObjectsPublished => Some(RequestErrorCode::InvalidRange),
            Self::InvalidRange => Some(RequestErrorCode::InvalidRange),
            Self::NoObjects => Some(RequestErrorCode::NoObjects),
            Self::MalformedTrack => Some(RequestErrorCode::MalformedTrack),
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            Self::TrackNotFound => "Track not found",
            Self::UnknownJoiningRequestId => "Unknown joining request id",
            Self::JoinedNotLargestObject => {
                "Joining FETCH of a subscription without the Largest Object filter"
            }
            Self::JoinedNotForwarding => "Joining FETCH of a subscription with Forward State 0",
            Self::JoiningRefused => "Joining FETCH refused",
            Self::NoObjectsPublished => "No objects published",
            Self::InvalidRange => "Invalid fetch range",
            Self::NoObjects => "No objects in fetch range",
            Self::MalformedTrack => "Malformed track",
        }
    }
}

impl From<JoiningFetchRefusal> for FetchError {
    fn from(refusal: JoiningFetchRefusal) -> Self {
        match refusal {
            JoiningFetchRefusal::NotLargestObjectFilter => Self::JoinedNotLargestObject,
            JoiningFetchRefusal::NotForwarding => Self::JoinedNotForwarding,
            _ => Self::JoiningRefused,
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
        let request_id = handler.request_id();
        let fill_budget = FillBudget::new(handler.fill_timeout(), self.fill_timeout);

        let (target, source) = match self.resolve_target_and_source(
            session_id,
            handler.as_ref(),
            table,
            cache_store,
        ) {
            Ok(resolved) => resolved,
            Err(err) => {
                let reason = err.reason().to_string();
                match err.code() {
                    Some(code) => {
                        let _ = handler.error(code, reason).await;
                    }
                    None => {
                        let _ = session_event_sender.send(
                            SessionEvent::protocol_violation_detected(session_id, reason),
                        );
                    }
                }
                return;
            }
        };
        let source = match source {
            FetchSource::Upstream if fill_budget.forbids_waiting() => {
                Self::serve_at_hand(&target, cache_store)
                    .map_or(FetchSource::Upstream, FetchSource::Cache)
            }
            source => source,
        };

        match source {
            FetchSource::Cache(CacheTarget {
                cache,
                start_location,
                end_location,
            }) => {
                // The newest live publisher's set, as in SUBSCRIBE_OK; the
                // cache keeps it for when no publisher is left.
                let track_properties = table
                    .get_upstream_track(&target.track_key)
                    .filter(|upstream_track| !upstream_track.subscriptions.is_empty())
                    .map_or_else(
                        || cache.track_properties(),
                        |upstream_track| upstream_track.track_properties(),
                    );
                // The relay does not yet track final End Of Track via PUBLISH_DONE,
                // so local cache responses cannot assert it.
                if let Err(e) = handler.ok(false, end_location, track_properties).await {
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
                    wait: fill_budget.delivery_wait(handler.marks_unknown_ranges()),
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
                    fill_budget,
                });
            }
        }
    }

    /// draft-14 §8.4 lets the relay send an upstream FETCH to any matching
    /// publisher. The publishers that answered SUBSCRIBE for the track are
    /// known to be alive, so they are tried first, newest first, and only when
    /// all of them fail are the other publishers of the track looked up.
    async fn create_upstream_fetch(
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        upstream_publisher_resolver: &UpstreamPublisherResolver,
        requester: SessionPeer,
        handler: &dyn FetchHandler,
        target: &FetchTarget,
    ) -> Result<PreparedUpstreamFetch, NoUpstreamFetch> {
        let mut tried = Vec::new();
        let mut last_error = None;
        for publisher_session_id in Self::subscribed_publishers(table, requester, target) {
            tried.push(publisher_session_id);
            match Self::fetch_from(forwarder, publisher_session_id, target).await {
                Ok(prepared) => return Ok(prepared),
                Err(err) if refused_as_malformed(&err) => {
                    return Err(Self::malformed_refusal(handler, publisher_session_id, &err).await);
                }
                Err(err) => last_error = Some(err),
            }
        }
        for publisher in upstream_publisher_resolver
            .resolve(
                table,
                &target.track_namespace,
                &target.track_name,
                requester,
            )
            .await
        {
            let Some(publisher_session_id) =
                upstream_publisher_resolver.session_of(&publisher).await
            else {
                continue;
            };
            if tried.contains(&publisher_session_id) {
                continue;
            }
            tried.push(publisher_session_id);
            match Self::fetch_from(forwarder, publisher_session_id, target).await {
                Ok(prepared) => return Ok(prepared),
                Err(err) if refused_as_malformed(&err) => {
                    return Err(Self::malformed_refusal(handler, publisher_session_id, &err).await);
                }
                Err(err) => last_error = Some(err),
            }
        }

        let (error_code, reason) = match last_error {
            Some(err) => Self::upstream_fetch_error_response(&err),
            None => {
                tracing::warn!(
                    track_namespace = %target.track_namespace,
                    track_name = %target.track_name,
                    "No upstream publisher found for fetch"
                );
                (
                    RequestErrorCode::TrackDoesNotExist,
                    FetchError::TrackNotFound.reason().to_string(),
                )
            }
        };
        let _ = handler.error(error_code, reason).await;
        Err(NoUpstreamFetch::Refused)
    }

    /// draft-18 §2.4.2: the track is malformed whoever else publishes it, so
    /// no other publisher is tried.
    async fn malformed_refusal(
        handler: &dyn FetchHandler,
        publisher_session_id: SessionId,
        error: &anyhow::Error,
    ) -> NoUpstreamFetch {
        let (error_code, reason) = Self::upstream_fetch_error_response(error);
        let _ = handler.error(error_code, reason).await;
        NoUpstreamFetch::MalformedTrack(publisher_session_id)
    }

    fn subscribed_publishers(
        table: &InMemoryLocalPubSubDirectory,
        requester: SessionPeer,
        target: &FetchTarget,
    ) -> Vec<SessionId> {
        table
            .get_upstream_track(&target.track_key)
            .map(|track| {
                track
                    .subscriptions
                    .into_iter()
                    .rev()
                    .filter(|(_, subscription)| {
                        requester == SessionPeer::Client
                            || subscription.publisher_peer == SessionPeer::Client
                    })
                    .map(|(publisher_session_id, _)| publisher_session_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn fetch_from(
        forwarder: &ControlMessageForwarder,
        publisher_session_id: SessionId,
        target: &FetchTarget,
    ) -> anyhow::Result<PreparedUpstreamFetch> {
        let fetched = forwarder
            .fetch(
                publisher_session_id,
                target.track_namespace.clone(),
                target.track_name.clone(),
                target.start_location,
                target.end_location,
                // The fill proves positions known in Location order; egress
                // serves the downstream's order. The downstream FILL_TIMEOUT
                // bounds this relay's wait and is not forwarded.
                moqt::fetch::FetchOption {
                    group_order: moqt::object::GroupOrder::Ascending,
                    ..moqt::fetch::FetchOption::default()
                },
            )
            .await;
        match fetched {
            Ok(handle) => {
                tracing::info!(
                    pub_session_id = publisher_session_id,
                    track_namespace = %target.track_namespace,
                    track_name = %target.track_name,
                    upstream_request_id = handle.request_id,
                    "Upstream FETCH_OK received; starting cache fill"
                );
                Ok(PreparedUpstreamFetch {
                    handle,
                    upstream_publisher_session_id: publisher_session_id,
                })
            }
            Err(err) => {
                tracing::warn!(
                    err = %format!("{err:#}"),
                    pub_session_id = publisher_session_id,
                    track_namespace = %target.track_namespace,
                    track_name = %target.track_name,
                    "Upstream FETCH failed"
                );
                Err(err)
            }
        }
    }

    fn upstream_fetch_error_response(error: &anyhow::Error) -> (RequestErrorCode, String) {
        match error.downcast_ref::<moqt::RequestFailure>() {
            Some(moqt::RequestFailure::Rejected { code, reason, .. }) => (*code, reason.clone()),
            Some(moqt::RequestFailure::Timeout) => (
                RequestErrorCode::Timeout,
                "Upstream fetch timed out".to_string(),
            ),
            _ => (
                RequestErrorCode::InternalError,
                "Internal relay error".to_string(),
            ),
        }
    }

    fn resolve_target_and_source(
        &self,
        session_id: SessionId,
        handler: &dyn FetchHandler,
        table: &InMemoryLocalPubSubDirectory,
        cache_store: &TrackCacheStore,
    ) -> Result<(FetchTarget, FetchSource), FetchError> {
        let target = self.resolve_fetch_target(session_id, handler, table)?;
        if cache_store
            .get(&target.track_key)
            .is_some_and(|cache| cache.is_malformed())
        {
            return Err(FetchError::MalformedTrack);
        }
        let upstream_largest =
            table
                .get_upstream_track(&target.track_key)
                .and_then(|upstream_track| match upstream_track.content_exists() {
                    ContentExists::True { location } => Some(location),
                    ContentExists::False => None,
                });
        let source = self.resolve_fetch_source(&target, cache_store, upstream_largest)?;
        Ok((target, source))
    }

    fn resolve_fetch_target(
        &self,
        session_id: SessionId,
        handler: &dyn FetchHandler,
        table: &InMemoryLocalPubSubDirectory,
    ) -> Result<FetchTarget, FetchError> {
        match handler.fetch_params() {
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
            } => self.resolve_joining_target(
                session_id,
                joining_request_id,
                handler,
                table,
                |largest| largest.group_id.saturating_sub(joining_start),
            ),
            FetchParams::AbsoluteJoining {
                joining_request_id,
                joining_start,
            } => {
                self.resolve_joining_target(session_id, joining_request_id, handler, table, |_| {
                    joining_start
                })
            }
        }
    }

    fn resolve_fetch_source(
        &self,
        target: &FetchTarget,
        cache_store: &TrackCacheStore,
        upstream_largest: Option<moqt::object::Location>,
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
        let source = match cache.resolve_fetch_range(start_location, end_location, upstream_largest)
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
                FetchSource::Upstream
            }
        };
        Ok(source)
    }

    /// A FETCH with FILL_TIMEOUT 0 whose range the cache does not cover is
    /// served with the cached objects and the rest reported as unknown
    /// (draft-18 §10.2.5 MUST NOT wait). Only a cache holding an object at
    /// or after the start knows the FETCH_OK End Location; otherwise the
    /// upstream FETCH_OK gives it and no object is waited for.
    fn serve_at_hand(target: &FetchTarget, cache_store: &TrackCacheStore) -> Option<CacheTarget> {
        let cache = cache_store.get(&target.track_key)?;
        let end_location = cache.known_end_location(target.start_location, target.end_location)?;
        Some(CacheTarget {
            cache,
            start_location: target.start_location,
            end_location,
        })
    }

    /// Checks a Joining FETCH in the order the drafts state: the joined
    /// subscription must exist (draft-14 §9.16.2, draft-18 §10.12.2), the
    /// session's draft must permit joining it, and the track must have
    /// objects (INVALID_RANGE).
    fn resolve_joining_target(
        &self,
        session_id: SessionId,
        joining_request_id: u64,
        handler: &dyn FetchHandler,
        table: &InMemoryLocalPubSubDirectory,
        start_group: impl FnOnce(moqt::object::Location) -> u64,
    ) -> Result<FetchTarget, FetchError> {
        // draft-18 §10.12.2 has pending REQUEST_UPDATEs of the subscription
        // applied first: the session worker handles a session's events in
        // order, so every update received before this FETCH already is.
        let Some((downstream_sub, delivery_control)) =
            table.get_downstream_subscription(session_id, joining_request_id)
        else {
            tracing::warn!(
                joining_request_id,
                "Joining fetch references unknown subscription"
            );
            return Err(FetchError::UnknownJoiningRequestId);
        };

        handler.check_joining(JoinedSubscription {
            filter_type: downstream_sub.filter_type,
            forward: delivery_control.forward,
        })?;
        let Some(largest) = downstream_sub.joining_location else {
            tracing::warn!("Joining fetch: no objects published at subscribe time");
            return Err(FetchError::NoObjectsPublished);
        };

        if table
            .get_upstream_track(&downstream_sub.track_key)
            .is_none()
        {
            tracing::warn!("Joined subscription has no active upstream subscription");
            return Err(FetchError::TrackNotFound);
        }

        let start_location = moqt::object::Location {
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
            end_location: largest.next(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_FETCH_FILL_TIMEOUT;
    use crate::modules::data_plane::{
        cache::track_cache::OpenSubgroupGuard, egress::coordinator::FetchWait,
    };
    use crate::modules::test_support::directory_fixtures::{
        delivery_control, downstream_subscription,
    };
    use crate::modules::{
        auth::{
            test_support::{app_token, connect_client, spawn_relay_and_connect_client},
            verified_token::VerifiedToken,
        },
        domain::{
            pub_sub_directory::entry::{
                ActiveUpstreamSubscription, DownstreamSubscription, UpstreamSubscriptionOrigin,
            },
            session_peer::SessionPeer,
        },
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, active_upstream, local_publisher_resolver, table_with_upstream,
                track_key, upstream_carrying,
            },
            mock_session::{
                FetchAnswer, MockFetchHandler, RecordedControlMessages,
                mock_session_answering_fetch, recorded_session_answering_fetch,
                session_repository_with_sessions,
            },
            relay_harness::fixtures::{
                cached_object::{insert_closed_group, open_group},
                location,
                subscription::unknown_track_properties,
            },
        },
    };
    use moqt::subscribe::FilterType;

    const FETCH: Fetch = Fetch {
        fill_timeout: DEFAULT_FETCH_FILL_TIMEOUT,
    };

    fn standalone_fetch_params(
        start_location: moqt::object::Location,
        end_location: moqt::object::Location,
    ) -> FetchParams {
        FetchParams::Standalone {
            track_namespace: vec!["ns".to_string()],
            track_name: "track".to_string(),
            start_location,
            end_location,
        }
    }

    #[test]
    fn fetch_goes_first_to_publishers_that_answered_subscribe_newest_first() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let track_key = TrackKey::new("ns", "track");
        for publisher_session_id in [1, 3] {
            table.register_upstream_subscription(
                track_key.clone(),
                publisher_session_id,
                active_upstream(UpstreamSubscriptionOrigin::Subscribe),
            );
        }
        let target = FETCH
            .resolve_fetch_target(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(
                    location(0, 0),
                    location(1, 0),
                )),
                &table,
            )
            .unwrap();

        // Act
        let publishers = Fetch::subscribed_publishers(&table, SessionPeer::Client, &target);

        // Assert
        assert_eq!(publishers, vec![3, 1]);
    }

    #[tokio::test]
    async fn fetch_goes_to_an_unsubscribed_publisher_after_the_subscribed_ones_refuse() {
        // Arrange
        const SUBSCRIBED_PUBLISHER: SessionId = 3;
        const OTHER_PUBLISHER: SessionId = 5;
        let table = InMemoryLocalPubSubDirectory::new();
        for publisher_session_id in [SUBSCRIBED_PUBLISHER, OTHER_PUBLISHER] {
            table.register_publish_namespace(
                publisher_session_id,
                "ns".to_string(),
                SessionPeer::Client,
            );
        }
        table.register_upstream_subscription(
            TrackKey::new("ns", "track"),
            SUBSCRIBED_PUBLISHER,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );
        let repository = session_repository_with_sessions(
            vec![
                (
                    SUBSCRIBED_PUBLISHER,
                    mock_session_answering_fetch(FetchAnswer::Refuse(
                        RequestErrorCode::TrackDoesNotExist,
                    )),
                ),
                (
                    OTHER_PUBLISHER,
                    mock_session_answering_fetch(FetchAnswer::FetchOk),
                ),
            ],
            VerifiedToken::full_access(),
        )
        .await;
        let target = FETCH
            .resolve_fetch_target(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(
                    location(0, 0),
                    location(1, 0),
                )),
                &table,
            )
            .unwrap();

        // Act
        let prepared = Fetch::create_upstream_fetch(
            &table,
            &ControlMessageForwarder { repository },
            &local_publisher_resolver(),
            SessionPeer::Client,
            &MockFetchHandler::default(),
            &target,
        )
        .await;

        // Assert
        assert_eq!(
            prepared.map(|prepared| prepared.upstream_publisher_session_id),
            Ok(OTHER_PUBLISHER)
        );
    }

    #[tokio::test]
    async fn fetch_refused_by_the_newest_publisher_goes_to_the_next() {
        // Arrange
        const REFUSING_PUBLISHER: SessionId = 5;
        const ANSWERING_PUBLISHER: SessionId = 3;
        let table = InMemoryLocalPubSubDirectory::new();
        for publisher_session_id in [ANSWERING_PUBLISHER, REFUSING_PUBLISHER] {
            table.register_publish_namespace(
                publisher_session_id,
                "ns".to_string(),
                SessionPeer::Client,
            );
        }
        let repository = session_repository_with_sessions(
            vec![
                (
                    REFUSING_PUBLISHER,
                    mock_session_answering_fetch(FetchAnswer::Refuse(
                        RequestErrorCode::TrackDoesNotExist,
                    )),
                ),
                (
                    ANSWERING_PUBLISHER,
                    mock_session_answering_fetch(FetchAnswer::FetchOk),
                ),
            ],
            VerifiedToken::full_access(),
        )
        .await;
        let target = FETCH
            .resolve_fetch_target(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(
                    location(0, 0),
                    location(1, 0),
                )),
                &table,
            )
            .unwrap();

        // Act
        let prepared = Fetch::create_upstream_fetch(
            &table,
            &ControlMessageForwarder { repository },
            &local_publisher_resolver(),
            SessionPeer::Client,
            &MockFetchHandler::default(),
            &target,
        )
        .await;

        // Assert
        assert_eq!(
            prepared.map(|prepared| prepared.upstream_publisher_session_id),
            Ok(ANSWERING_PUBLISHER)
        );
    }

    struct HandledFetch {
        oks: Vec<moqt::object::Location>,
        handed_over: Option<EgressFetchRequest>,
        upstream: RecordedControlMessages,
    }

    /// Handles a FETCH of {0,0}..{2,0} with `fill_timeout` from session 2,
    /// whose only publisher, session 5, answers `upstream`.
    async fn handle_fetch_with_fill_timeout(
        fill_timeout: Duration,
        cache_store: Arc<TrackCacheStore>,
        upstream: FetchAnswer,
    ) -> HandledFetch {
        let handler = MockFetchHandler {
            fill_timeout: Some(fill_timeout),
            ..MockFetchHandler::fetching(standalone_fetch_params(location(0, 0), location(2, 0)))
        };
        handle_fetch(handler, cache_store, upstream).await
    }

    /// Handles `handler`'s FETCH from session 2, whose only publisher,
    /// session 5, answers `upstream`.
    async fn handle_fetch(
        handler: MockFetchHandler,
        cache_store: Arc<TrackCacheStore>,
        upstream: FetchAnswer,
    ) -> HandledFetch {
        const PUBLISHER: SessionId = 5;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(PUBLISHER, "ns".to_string(), SessionPeer::Client);
        let (publisher, recorded) = recorded_session_answering_fetch(upstream);
        let repository = session_repository_with_sessions(
            vec![(PUBLISHER, publisher)],
            VerifiedToken::full_access(),
        )
        .await;
        let oks = handler.oks.clone();
        let (egress_sender, mut egress_receiver) = tokio::sync::mpsc::channel(1);
        let (session_event_sender, _session_event_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        FETCH
            .handle(
                2,
                &Span::none(),
                &table,
                &cache_store,
                &egress_sender,
                &session_event_sender,
                &ControlMessageForwarder { repository },
                &Arc::new(local_publisher_resolver()),
                Box::new(handler),
            )
            .await;
        let handed_over = tokio::time::timeout(Duration::from_secs(1), egress_receiver.recv())
            .await
            .ok()
            .flatten()
            .map(|command| match command {
                EgressCommand::StartFetch(request) => request,
                EgressCommand::StartReader(_) => panic!("expected a fetch"),
            });
        let oks = oks.lock().unwrap().clone();
        HandledFetch {
            oks,
            handed_over,
            upstream: recorded,
        }
    }

    #[tokio::test]
    async fn a_zero_fill_timeout_serves_the_cache_without_an_upstream_fetch() {
        // Arrange: only group 2 is cached; the publisher never answers FETCH
        let cache_store = Arc::new(TrackCacheStore::new());
        insert_closed_group(
            &cache_store.get_or_create(&TrackKey::new("ns", "track")),
            2,
            &[0],
        );

        // Act
        let handled =
            handle_fetch_with_fill_timeout(Duration::ZERO, cache_store, FetchAnswer::Never).await;

        // Assert: draft-18 §10.2.5 MUST NOT wait for upstream delivery
        assert_eq!(handled.oks, vec![location(2, 0)]);
        assert!(
            handled
                .handed_over
                .is_some_and(|request| request.wait == FetchWait::Never),
            "the cached objects are served at once, the rest reported unknown"
        );
    }

    #[tokio::test]
    async fn a_zero_fill_timeout_without_a_cache_waits_for_no_upstream_object() {
        // Act: the publisher answers FETCH_OK but never sends an object
        let handled = handle_fetch_with_fill_timeout(
            Duration::ZERO,
            Arc::new(TrackCacheStore::new()),
            FetchAnswer::FetchOk,
        )
        .await;

        // Assert: draft-18 §10.2.5, FETCH_OK alone tells the End Location
        assert_eq!(handled.oks, vec![location(2, 0)]);
        assert!(
            handled
                .handed_over
                .is_some_and(|request| request.wait == FetchWait::Never)
        );
    }

    /// Fills `cache` so that a FETCH of {0,0}..{2,0} is served from it,
    /// waiting for the rest of group 0, which live ingest holds open while
    /// the guard lives.
    fn hold_group_0_open(cache: &TrackCache) -> OpenSubgroupGuard<'_> {
        cache.begin_live_ingest();
        let group0 = open_group(cache, 0, &[0, 1]);
        insert_closed_group(cache, 1, &[0]);
        group0
    }

    #[tokio::test]
    async fn a_zero_fill_timeout_serves_an_open_group_without_waiting_for_its_tail() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let cache = cache_store.get_or_create(&TrackKey::new("ns", "track"));
        let _group0 = hold_group_0_open(&cache);

        // Act
        let handled =
            handle_fetch_with_fill_timeout(Duration::ZERO, cache_store, FetchAnswer::Never).await;

        // Assert: draft-18 §10.2.5 MUST NOT wait for upstream delivery
        assert_eq!(handled.oks, vec![location(1, 1)]);
        assert!(
            handled
                .handed_over
                .is_some_and(|request| request.wait == FetchWait::Never)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_fill_timeout_bounds_the_wait_of_a_fetch_served_from_the_cache() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let cache = cache_store.get_or_create(&TrackKey::new("ns", "track"));
        let _group0 = hold_group_0_open(&cache);
        let arrival = tokio::time::Instant::now();

        // Act
        let handled = handle_fetch_with_fill_timeout(
            Duration::from_millis(500),
            cache_store,
            FetchAnswer::Never,
        )
        .await;

        // Assert: draft-18 §10.2.5, the budget counts from the FETCH's arrival
        assert_eq!(handled.oks, vec![location(1, 1)]);
        assert_eq!(
            handled.handed_over.map(|request| request.wait),
            Some(FetchWait::Until(arrival + Duration::from_millis(500)))
        );
    }

    /// A FETCH of {0,0}..{2,0} without FILL_TIMEOUT from a session whose
    /// response can mark objects unknown or not.
    fn fetch_without_fill_timeout(marks_unknown_ranges: bool) -> MockFetchHandler {
        MockFetchHandler {
            marks_unknown_ranges,
            ..MockFetchHandler::fetching(standalone_fetch_params(location(0, 0), location(2, 0)))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_draft18_fetch_without_fill_timeout_waits_for_the_cache_up_to_the_relay_limit() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let cache = cache_store.get_or_create(&TrackKey::new("ns", "track"));
        let _group0 = hold_group_0_open(&cache);
        let arrival = tokio::time::Instant::now();

        // Act
        let handled = handle_fetch(
            fetch_without_fill_timeout(true),
            cache_store,
            FetchAnswer::Never,
        )
        .await;

        // Assert: draft-18 §10.2.5, the relay waits an implementation specific duration, then reports unknown
        assert_eq!(handled.oks, vec![location(1, 1)]);
        assert_eq!(
            handled.handed_over.map(|request| request.wait),
            Some(FetchWait::Until(arrival + DEFAULT_FETCH_FILL_TIMEOUT))
        );
    }

    #[tokio::test]
    async fn a_draft14_fetch_waits_for_the_cache_until_its_objects_arrive() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let cache = cache_store.get_or_create(&TrackKey::new("ns", "track"));
        let _group0 = hold_group_0_open(&cache);

        // Act
        let handled = handle_fetch(
            fetch_without_fill_timeout(false),
            cache_store,
            FetchAnswer::Never,
        )
        .await;

        // Assert: draft-14 §9.16 cannot report objects unknown, so the delivery waits for them
        assert_eq!(handled.oks, vec![location(1, 1)]);
        assert_eq!(
            handled.handed_over.map(|request| request.wait),
            Some(FetchWait::Unlimited)
        );
    }

    #[tokio::test]
    async fn a_fetch_forwarded_upstream_carries_no_fill_timeout() {
        // Act: nothing is cached, so the FETCH goes upstream
        let handled = handle_fetch_with_fill_timeout(
            Duration::from_millis(500),
            Arc::new(TrackCacheStore::new()),
            FetchAnswer::FetchOk,
        )
        .await;

        // Assert: draft-18 §10.2.5, the FILL_TIMEOUT bounds this relay's wait;
        // an upstream relay waits as long as it decides itself
        assert_eq!(handled.upstream.fetch_fill_timeouts(), vec![None]);
    }

    #[test]
    fn upstream_timeout_maps_to_fetch_error_timeout() {
        // Arrange
        let error = anyhow::Error::new(moqt::RequestFailure::Timeout);

        // Act
        let (code, _reason) = Fetch::upstream_fetch_error_response(&error);

        // Assert
        assert_eq!(code, RequestErrorCode::Timeout);
    }

    #[test]
    fn upstream_request_error_code_is_relayed_verbatim() {
        // Arrange
        let error = anyhow::Error::new(moqt::RequestFailure::Rejected {
            request_id: 7,
            code: RequestErrorCode::NotSupported,
            reason: "not supported".to_string(),
        });

        // Act
        let (code, reason) = Fetch::upstream_fetch_error_response(&error);

        // Assert
        assert_eq!(code, RequestErrorCode::NotSupported);
        assert_eq!(reason, "not supported");
    }

    #[test]
    fn fetch_source_is_upstream_when_track_entry_missing() {
        // Arrange
        let cache_store = Arc::new(TrackCacheStore::new());
        let start = moqt::object::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::object::Location {
            group_id: 0,
            object_id: 1,
        };

        // Act
        let (target, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(start, end)),
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
        let start = moqt::object::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::object::Location {
            group_id: 0,
            object_id: 1,
        };

        // Act
        let (_, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(start, end)),
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
        let start = moqt::object::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::object::Location {
            group_id: 0,
            object_id: 3,
        };

        // Act
        let (_, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(start, end)),
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
        let start = moqt::object::Location {
            group_id: 0,
            object_id: 0,
        };
        let end = moqt::object::Location {
            group_id: 1,
            object_id: 2,
        };

        // Act
        let (target, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(start, end)),
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
        let start = moqt::object::Location {
            group_id: 0,
            object_id: 0,
        };
        let requested_end = moqt::object::Location {
            group_id: 2,
            object_id: 0,
        };

        // Act
        let (_, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(start, requested_end)),
                &InMemoryLocalPubSubDirectory::new(),
                &cache_store,
            )
            .unwrap();

        // Assert
        match source {
            FetchSource::Cache(resolved) => {
                assert_eq!(
                    resolved.end_location,
                    moqt::object::Location {
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
        let result = FETCH.resolve_target_and_source(
            2,
            &MockFetchHandler::fetching(standalone_fetch_params(
                moqt::object::Location {
                    group_id: 0,
                    object_id: 1,
                },
                moqt::object::Location {
                    group_id: 0,
                    object_id: 2,
                },
            )),
            &InMemoryLocalPubSubDirectory::new(),
            &cache_store,
        );

        // Assert
        assert!(matches!(result, Err(FetchError::InvalidRange)));
    }

    #[test]
    fn a_fetch_of_the_upstream_largest_group_not_yet_cached_goes_upstream() {
        // Arrange: the upstream SUBSCRIBE_OK reported Largest {1, 3}; live
        // ingest runs, but the cache holds only group 0
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            ActiveUpstreamSubscription {
                content_exists: ContentExists::True {
                    location: location(1, 3),
                },
                ..active_upstream(UpstreamSubscriptionOrigin::Subscribe)
            },
        );
        let cache_store = Arc::new(TrackCacheStore::new());
        let cache = cache_store.get_or_create(&track_key());
        insert_closed_group(&cache, 0, &[0]);
        cache.begin_live_ingest();

        // Act
        let result = FETCH.resolve_target_and_source(
            2,
            &MockFetchHandler::fetching(standalone_fetch_params(location(1, 0), location(1, 4))),
            &table,
            &cache_store,
        );

        // Assert: the start is not after the Largest Object, so no INVALID_RANGE (draft-14 §9.16.3, draft-18 §10.12.3)
        assert!(matches!(result, Ok((_, FetchSource::Upstream))));
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
        let result = FETCH.resolve_target_and_source(
            2,
            &MockFetchHandler::fetching(standalone_fetch_params(
                moqt::object::Location {
                    group_id: 1,
                    object_id: 0,
                },
                moqt::object::Location {
                    group_id: 1,
                    object_id: 0,
                },
            )),
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
        let start = moqt::object::Location {
            group_id: 1,
            object_id: 2,
        };
        let end = moqt::object::Location {
            group_id: 1,
            object_id: 4,
        };

        // Act
        let (_, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(standalone_fetch_params(start, end)),
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

    /// The track properties of each FETCH_OK sent for a FETCH that the
    /// cache of `ns/track` serves, which served `cached` from publisher 1
    /// until it left.
    async fn fetch_oks_from_cache(
        table: InMemoryLocalPubSubDirectory,
        cached: moqt::TrackProperties,
    ) -> Vec<moqt::TrackProperties> {
        let cache_store = Arc::new(TrackCacheStore::new());
        let cache = cache_store.get_or_create(&TrackKey::new("ns", "track"));
        let _open_g1 = open_group(&cache, 1, &[2, 3]);
        cache.publisher_started(1, cached);
        cache.publisher_stopped(1);
        let (egress_sender, _egress_receiver) = tokio::sync::mpsc::channel(1);
        let (session_event_sender, _session_event_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let repository =
            session_repository_with_sessions(vec![], VerifiedToken::full_access()).await;
        let handler =
            MockFetchHandler::fetching(standalone_fetch_params(location(1, 2), location(1, 4)));
        FETCH
            .handle(
                2,
                &Span::none(),
                &Arc::new(table),
                &cache_store,
                &egress_sender,
                &session_event_sender,
                &ControlMessageForwarder { repository },
                &Arc::new(local_publisher_resolver()),
                Box::new(handler.clone()),
            )
            .await;
        handler.fetch_oks.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_fetch_served_from_cache_after_the_publisher_left_carries_the_cached_properties() {
        // Act: the directory holds no upstream subscription any more
        let fetch_oks = fetch_oks_from_cache(
            InMemoryLocalPubSubDirectory::new(),
            unknown_track_properties(1),
        )
        .await;

        // Assert: draft-18 §2.5 caches the properties with the track
        assert_eq!(fetch_oks, vec![unknown_track_properties(1)]);
    }

    #[tokio::test]
    async fn a_fetch_served_from_cache_carries_the_newest_live_publishers_properties() {
        // Arrange: publisher 3 answered SUBSCRIBE, but the cache has served
        // only publisher 1's set
        let table = InMemoryLocalPubSubDirectory::new();
        for publisher_session_id in [1, 3] {
            table.register_upstream_subscription(
                TrackKey::new("ns", "track"),
                publisher_session_id,
                upstream_carrying(unknown_track_properties(publisher_session_id)),
            );
        }

        // Act
        let fetch_oks = fetch_oks_from_cache(table, unknown_track_properties(1)).await;

        // Assert: the set a SUBSCRIBE_OK for the track carries
        assert_eq!(fetch_oks, vec![unknown_track_properties(3)]);
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
        let result = FETCH.resolve_fetch_target(
            1,
            &MockFetchHandler::fetching(relative_joining_fetch_params(999)),
            &table,
        );

        // Assert
        assert!(matches!(result, Err(FetchError::UnknownJoiningRequestId)));
    }

    #[test]
    fn resolve_joining_target_no_objects_published() {
        // Arrange
        let (table, key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(
            2,
            100,
            SessionPeer::Client,
            downstream_subscription(key, None),
            delivery_control(true),
        );

        // Act
        let result = FETCH.resolve_fetch_target(
            2,
            &MockFetchHandler::fetching(relative_joining_fetch_params(100)),
            &table,
        );

        // Assert
        assert!(matches!(result, Err(FetchError::NoObjectsPublished)));
    }

    /// draft-18 §10.12.2, as the session's moqt handler applies it.
    fn draft18_joining_rule(subscription: JoinedSubscription) -> Result<(), JoiningFetchRefusal> {
        if subscription.forward {
            Ok(())
        } else {
            Err(JoiningFetchRefusal::NotForwarding)
        }
    }

    fn draft18_joining_fetch() -> MockFetchHandler {
        MockFetchHandler {
            joining_rule: draft18_joining_rule,
            ..MockFetchHandler::fetching(relative_joining_fetch_params(100))
        }
    }

    fn register_joined(
        table: &InMemoryLocalPubSubDirectory,
        subscription: DownstreamSubscription,
        forward: bool,
    ) {
        table.register_downstream_subscription(
            2,
            100,
            SessionPeer::Client,
            subscription,
            delivery_control(forward),
        );
    }

    #[test]
    fn a_joining_fetch_of_a_subscription_at_forward_0_gets_invalid_range() {
        // Arrange
        let (table, key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        register_joined(
            &table,
            downstream_subscription(key, Some(location(3, 0))),
            false,
        );

        // Act
        let result = FETCH.resolve_fetch_target(2, &draft18_joining_fetch(), &table);

        // Assert: draft-18 §10.12.2
        let Err(error) = result else {
            panic!("the Joining FETCH should be refused");
        };
        assert!(
            matches!(error, FetchError::JoinedNotForwarding),
            "{error:?}"
        );
        assert_eq!(error.code(), Some(RequestErrorCode::InvalidRange));
    }

    #[test]
    fn a_joining_fetch_the_draft_permits_joins_a_next_group_start_subscription() {
        // Arrange
        let (table, key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let subscription = DownstreamSubscription {
            filter_type: FilterType::NextGroupStart,
            ..downstream_subscription(key, Some(location(3, 0)))
        };
        register_joined(&table, subscription, true);

        // Act
        let target = FETCH
            .resolve_fetch_target(2, &draft18_joining_fetch(), &table)
            .unwrap();

        // Assert
        assert_eq!(target.end_location, location(3, 1));
    }

    #[test]
    fn the_refusal_of_the_draft_comes_before_a_track_without_objects() {
        // Arrange
        let (table, key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        register_joined(&table, downstream_subscription(key, None), false);

        // Act
        let result = FETCH.resolve_fetch_target(2, &draft18_joining_fetch(), &table);

        // Assert
        assert!(
            matches!(result, Err(FetchError::JoinedNotForwarding)),
            "{:?}",
            result.err()
        );
    }

    #[test]
    fn an_unknown_joining_request_id_comes_before_the_refusal_of_the_draft() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let handler = MockFetchHandler {
            joining_rule: |_| Err(JoiningFetchRefusal::NotLargestObjectFilter),
            ..MockFetchHandler::fetching(relative_joining_fetch_params(100))
        };

        // Act
        let result = FETCH.resolve_fetch_target(2, &handler, &table);

        // Assert
        assert!(
            matches!(result, Err(FetchError::UnknownJoiningRequestId)),
            "{:?}",
            result.err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn joining_fetch_of_a_next_group_start_subscription_closes_the_session() {
        // Arrange: a publisher serves one object through the relay
        let (relay, publisher) =
            spawn_relay_and_connect_client(app_token(Some(""), Some(""))).await;
        let origin = moqt::publish::Origin::default();
        origin
            .ensure_track("APP/ns", "track", 0, moqt::publish::Retention::NewestOnly)
            .await;
        origin
            .write(
                "APP/ns",
                "track",
                moqt::publish::OutgoingObject::plain(
                    moqt::publish::GroupBoundary::Next,
                    bytes::Bytes::from_static(b"o0"),
                ),
            )
            .await
            .unwrap();
        publisher
            .publisher()
            .publish_namespace("APP/ns".to_string())
            .await
            .unwrap();
        let serving = origin.clone();
        let publisher_events = tokio::spawn(async move {
            while let Ok(event) = publisher.receive_event().await {
                serving.handle(event).await;
            }
        });
        let subscriber = connect_client(&relay).await;
        let subscription = subscriber
            .subscriber()
            .subscribe(
                "APP/ns".to_string(),
                "track".to_string(),
                moqt::subscribe::SubscribeOption {
                    filter_type: moqt::subscribe::FilterType::NextGroupStart,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // Act
        let _ = subscriber
            .subscriber()
            .fetch_relative_joining(&subscription, 0, moqt::fetch::FetchOption::default())
            .await;
        let event = tokio::time::timeout(Duration::from_secs(5), subscriber.receive_event())
            .await
            .expect("the relay kept the session open")
            .unwrap();

        // Assert: the peer of a closed session sees the close as a disconnection
        assert!(
            matches!(event, moqt::SessionEvent::Disconnected()),
            "{event:?}"
        );
        publisher_events.abort();
    }

    #[test]
    fn resolve_joining_target_ends_after_stored_largest() {
        // Arrange
        let largest = moqt::object::Location {
            group_id: 10,
            object_id: 5,
        };
        let (table, key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.register_downstream_subscription(
            2,
            100,
            SessionPeer::Client,
            downstream_subscription(key, Some(largest)),
            delivery_control(true),
        );

        // Act
        let target = FETCH
            .resolve_fetch_target(
                2,
                &MockFetchHandler::fetching(relative_joining_fetch_params(100)),
                &table,
            )
            .unwrap();

        // Assert
        assert_eq!(target.track_key, TrackKey::new("ns", "track"));
        assert_eq!(target.track_namespace, "ns");
        assert_eq!(target.track_name, "track");
        assert_eq!(
            target.end_location,
            moqt::object::Location {
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
            SessionPeer::Client,
            downstream_subscription(
                upstream_key,
                Some(moqt::object::Location {
                    group_id: 1,
                    object_id: 1,
                }),
            ),
            delivery_control(true),
        );
        let _open_g1 = open_group(&cache, 1, &[0, 1]);

        // Act
        let (target, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(FetchParams::RelativeJoining {
                    joining_request_id: 100,
                    joining_start: 1,
                }),
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
                    moqt::object::Location {
                        group_id: 0,
                        object_id: 0
                    }
                );
                assert_eq!(
                    target.end_location,
                    moqt::object::Location {
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
            SessionPeer::Client,
            downstream_subscription(
                upstream_key,
                Some(moqt::object::Location {
                    group_id: 2,
                    object_id: 3,
                }),
            ),
            delivery_control(true),
        );

        // Act
        let (target, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(FetchParams::AbsoluteJoining {
                    joining_request_id: 100,
                    joining_start: 1,
                }),
                &table,
                &cache_store,
            )
            .unwrap();

        // Assert: start = {joining_start, 0}, end = largest + 1 (Standalone encoding, §9.16.2.1)
        match source {
            FetchSource::Upstream => {
                assert_eq!(
                    target.start_location,
                    moqt::object::Location {
                        group_id: 1,
                        object_id: 0
                    }
                );
                assert_eq!(
                    target.end_location,
                    moqt::object::Location {
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
            SessionPeer::Client,
            downstream_subscription(
                upstream_key,
                Some(moqt::object::Location {
                    group_id: 1,
                    object_id: 1,
                }),
            ),
            delivery_control(true),
        );

        // Act
        let (target, source) = FETCH
            .resolve_target_and_source(
                2,
                &MockFetchHandler::fetching(FetchParams::RelativeJoining {
                    joining_request_id: 100,
                    joining_start: 5,
                }),
                &table,
                &cache_store,
            )
            .unwrap();

        // Assert
        match source {
            FetchSource::Upstream => {
                assert_eq!(
                    target.start_location,
                    moqt::object::Location {
                        group_id: 0,
                        object_id: 0
                    }
                );
                assert_eq!(
                    target.end_location,
                    moqt::object::Location {
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
            SessionPeer::Client,
            downstream_subscription(
                upstream_key,
                Some(moqt::object::Location {
                    group_id: 1,
                    object_id: 1,
                }),
            ),
            delivery_control(true),
        );

        // Act
        let result = FETCH.resolve_target_and_source(
            2,
            &MockFetchHandler::fetching(FetchParams::AbsoluteJoining {
                joining_request_id: 100,
                joining_start: 2,
            }),
            &table,
            &cache_store,
        );

        // Assert
        assert!(matches!(result, Err(FetchError::InvalidRange)));
    }
}

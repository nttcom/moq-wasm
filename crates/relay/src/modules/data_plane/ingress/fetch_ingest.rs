use moqt::object::DataStreamResetCode;
use std::{sync::Arc, time::Duration};

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    data_plane::{
        cache::{
            cached_object::{CacheStamp, CachedObject},
            track_cache::TrackCache,
        },
        egress::coordinator::{EgressCommand, EgressFetchRequest},
    },
    domain::{session_id::SessionId, track_key::TrackKey},
    session::{session_event::SessionEvent, session_repository::SessionRepository},
};

pub(crate) struct FetchIngestStart {
    pub(crate) track_key: TrackKey,
    pub(crate) upstream_publisher_session_id: SessionId,
    pub(crate) fetch_handle: moqt::fetch::FetchHandle,
    pub(crate) egress_start: EgressFetchRequest,
    pub(crate) fill_timeout: Duration,
}

/// Why a fill did not hand its range to the downstream FETCH.
#[derive(Debug, thiserror::Error)]
enum FillFailure {
    /// draft-18 §11.4.4.2: the objects may exist, so the range is not known.
    #[error("the upstream FETCH response left objects of unknown status")]
    UnknownObjects,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Strict store-and-forward: delivery starts only after the whole response
/// reached `Fetch::End`, so first-byte latency equals the upstream transfer
/// time and fills longer than MOQT_FETCH_FILL_TIMEOUT_SECS fail.
pub(crate) struct FetchIngest {
    _join_handle: JoinHandle<()>,
}

impl FetchIngest {
    pub(crate) fn run(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: tokio::sync::mpsc::Sender<EgressCommand>,
        session_event_sender: mpsc::UnboundedSender<SessionEvent>,
        start: FetchIngestStart,
    ) -> Self {
        let downstream_subscriber_session_id = start.egress_start.subscriber_session_id;
        let request_id = start.egress_start.request_id;
        let upstream_publisher_session_id = start.upstream_publisher_session_id;
        let upstream_fetch = start.fetch_handle.clone();
        let track_key = start.track_key.clone();
        let cache = start.egress_start.cache.clone();
        let join_handle = tokio::spawn(async move {
            if let Err(error) = Self::run_inner(session_repo.clone(), &egress_sender, start).await {
                let error_code = if let FillFailure::UnknownObjects = error {
                    tracing::warn!(%error, "fetch ingest cannot serve the range");
                    DataStreamResetCode::UnknownObjectStatus
                } else if cache.is_malformed() {
                    tracing::error!(error = %format!("{error:#}"), "fetch ingest failed");
                    let _ = session_event_sender.send(SessionEvent::malformed_track_detected(
                        upstream_publisher_session_id,
                        track_key,
                    ));
                    Self::cancel_upstream_fetch(
                        session_repo.clone(),
                        upstream_publisher_session_id,
                        &upstream_fetch,
                    )
                    .await;
                    DataStreamResetCode::MalformedTrack
                } else {
                    // Expected request-scoped failures (timeout, upstream
                    // reset): log the chain without anyhow's captured backtrace.
                    tracing::error!(error = %format!("{error:#}"), "fetch ingest failed");
                    DataStreamResetCode::InternalError
                };
                Self::reset_downstream_fetch(
                    session_repo,
                    downstream_subscriber_session_id,
                    request_id,
                    error_code,
                )
                .await;
            }
        });
        Self {
            _join_handle: join_handle,
        }
    }

    async fn run_inner(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        start: FetchIngestStart,
    ) -> Result<(), FillFailure> {
        let timeout = start.fill_timeout;
        tokio::time::timeout(
            timeout,
            Self::ingest_fetch_stream(session_repo, egress_sender, start),
        )
        .await
        .map_err(|_| anyhow::anyhow!("fetch ingest timed out after {:?}", timeout))?
    }

    async fn ingest_fetch_stream(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        start: FetchIngestStart,
    ) -> Result<(), FillFailure> {
        let (mut subscriber, persist) = {
            let session_repo = session_repo.lock().await;
            let subscriber = session_repo
                .subscriber(start.upstream_publisher_session_id)
                .ok_or_else(|| anyhow::anyhow!("upstream publisher session not found"))?;
            (
                subscriber,
                session_repo.is_client_session(start.upstream_publisher_session_id),
            )
        };
        let mut receiver = subscriber
            .create_fetch_receiver(&start.fetch_handle)
            .await?;
        let cache = start.egress_start.cache.clone();
        let start_eviction_generation = cache.eviction_generation();
        let stamp = CacheStamp {
            max_cache_duration: start
                .fetch_handle
                .max_cache_duration
                .map(Duration::from_millis),
            persist,
        };

        let mut unknown_objects = false;
        loop {
            let received = tokio::select! {
                received = receiver.receive() => received.inspect_err(|error| {
                    if error.is::<moqt::object::MalformedTrackError>() {
                        cache.mark_malformed();
                    }
                })?,
                _ = cache.malformed_track_detected() => {
                    return Err(anyhow::anyhow!("malformed track detected while awaiting fetch data").into());
                }
            };
            match received {
                moqt::fetch::Fetch::Object(object) => {
                    Self::append_fetch_object(&cache, object, stamp)?;
                }
                // The known objects around it are still cached.
                moqt::fetch::Fetch::UnknownRange { .. } => unknown_objects = true,
                moqt::fetch::Fetch::End => {
                    if cache.eviction_generation() != start_eviction_generation {
                        tracing::warn!(
                            request_id = start.egress_start.request_id,
                            "fetch fill crossed cache eviction; resetting downstream fetch"
                        );
                        return Err(anyhow::anyhow!("fetch fill crossed cache eviction").into());
                    }
                    if unknown_objects {
                        return Err(FillFailure::UnknownObjects);
                    }
                    cache.insert_fetch_known_range(
                        start.egress_start.start_location,
                        start.egress_start.end_location,
                    );
                    egress_sender
                        .send(EgressCommand::StartFetch(start.egress_start))
                        .await
                        .map_err(anyhow::Error::from)?;
                    return Ok(());
                }
            }
        }
    }

    fn append_fetch_object(
        cache: &TrackCache,
        object: moqt::object::Object,
        stamp: CacheStamp,
    ) -> anyhow::Result<()> {
        let (group_id, object_id) = (object.location.group_id, object.location.object_id);
        cache
            .insert(stamp.apply(CachedObject::from_object(object)))
            .map_err(|_| {
                anyhow::anyhow!(
                    "malformed track detected during fetch fill (group {group_id}, object {object_id})"
                )
            })
    }

    async fn cancel_upstream_fetch(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        upstream_publisher_session_id: SessionId,
        upstream_fetch: &moqt::fetch::FetchHandle,
    ) {
        let upstream_request_id = upstream_fetch.request_id;
        let subscriber = {
            let session_repo = session_repo.lock().await;
            session_repo.subscriber(upstream_publisher_session_id)
        };
        let Some(subscriber) = subscriber else {
            return;
        };
        if let Err(error) = subscriber.send_fetch_cancel(upstream_fetch).await {
            tracing::warn!(
                error = %format!("{error:#}"),
                upstream_request_id,
                "failed to send upstream FETCH_CANCEL"
            );
        } else {
            tracing::info!(upstream_request_id, "sent upstream FETCH_CANCEL");
        }
    }

    async fn reset_downstream_fetch(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        downstream_subscriber_session_id: SessionId,
        request_id: u64,
        error_code: DataStreamResetCode,
    ) {
        let publisher = {
            let session_repo = session_repo.lock().await;
            session_repo.publisher(downstream_subscriber_session_id)
        };
        let Some(publisher) = publisher else {
            return;
        };
        // FETCH_OK has already been sent. FIN would assert that every available object
        // was delivered, so failure after FETCH_OK must be signaled with RESET_STREAM.
        match publisher.new_fetch_sender(request_id).await {
            Ok(sender) => {
                if let Err(error) = sender.reset(error_code).await {
                    tracing::error!(error = %format!("{error:#}"), "failed to reset downstream fetch stream");
                }
            }
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "failed to create downstream fetch stream")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_FETCH_FILL_TIMEOUT;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        data_plane::cache::track_cache::FetchRangeResolution,
        session::session_event::EventKind,
        test_support::{
            mock_session::{
                FetchAnswer, mock_session_answering_fetch, session_repository_with_session,
                session_repository_with_upstream_session,
            },
            relay_harness::fixtures::{
                cached_object::{status_object, stream_object, stream_object_with_payload},
                location,
            },
        },
    };
    use bytes::Bytes;
    use moqt::object::ObjectStatus;

    fn fill(cache: &TrackCache, object: CachedObject) -> anyhow::Result<()> {
        FetchIngest::append_fetch_object(cache, object.object.clone(), CacheStamp::default())
    }

    #[tokio::test]
    async fn malformed_track_cancels_upstream_fetch_while_awaiting_data() {
        // Arrange: the track is latched malformed by conflicting duplicate objects.
        const UPSTREAM_SESSION: SessionId = 1;
        const DOWNSTREAM_SESSION: SessionId = 2; // not registered: downstream reset is skipped
        const UPSTREAM_FETCH_REQUEST_ID: u64 = 7;
        let (session_repo, recorded) =
            session_repository_with_upstream_session(UPSTREAM_SESSION).await;

        let cache = Arc::new(TrackCache::new());
        fill(
            &cache,
            stream_object_with_payload(0, 0, Bytes::from_static(b"a")),
        )
        .unwrap();
        let _ = fill(
            &cache,
            stream_object_with_payload(0, 0, Bytes::from_static(b"b")),
        );
        assert!(cache.is_malformed());

        let (egress_sender, _egress_receiver) = mpsc::channel(1);
        let (session_event_sender, mut session_event_receiver) = mpsc::unbounded_channel();
        let start = FetchIngestStart {
            track_key: TrackKey::new("ns", "track"),
            upstream_publisher_session_id: UPSTREAM_SESSION,
            fetch_handle: moqt::fetch::FetchHandle {
                request_id: UPSTREAM_FETCH_REQUEST_ID,
                group_order: moqt::object::GroupOrder::Ascending,
                end_of_track: false,
                end_location: location(0, 1),
                max_cache_duration: None,
                remainder: None,
            },
            egress_start: EgressFetchRequest {
                subscriber_session_id: DOWNSTREAM_SESSION,
                request_id: 3,
                cache: cache.clone(),
                start_location: location(0, 0),
                end_location: location(0, 1),
                group_order: moqt::object::GroupOrder::Ascending,
            },
            fill_timeout: DEFAULT_FETCH_FILL_TIMEOUT,
        };

        // Act: the receiver pends forever, so only the malformed latch can
        // make the ingest bail.
        let _ingest = FetchIngest::run(session_repo, egress_sender, session_event_sender, start);

        // Assert
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            if !recorded
                .fetch_cancelled_request_ids
                .lock()
                .unwrap()
                .is_empty()
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "upstream FETCH_CANCEL was never sent"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(
            *recorded.fetch_cancelled_request_ids.lock().unwrap(),
            vec![UPSTREAM_FETCH_REQUEST_ID]
        );
        // Assert: the detection is reported for upstream subscription teardown.
        let event = session_event_receiver
            .try_recv()
            .expect("malformed detection should be reported");
        assert_eq!(event.session_id, UPSTREAM_SESSION);
        assert!(matches!(
            event.kind,
            EventKind::MalformedTrackDetected(track_key) if track_key == TrackKey::new("ns", "track")
        ));
    }

    #[tokio::test]
    async fn fetch_status_object_does_not_close_subgroup() {
        // Arrange: a FETCH response can contain EndOfGroup status, but that
        // must not close the shared live GroupCache.
        let cache = Arc::new(TrackCache::new());
        fill(&cache, status_object(0, 0, ObjectStatus::EndOfGroup)).unwrap();
        fill(&cache, status_object(1, 0, ObjectStatus::Normal)).unwrap();

        // Act
        let resolution = cache.resolve_fetch_range(location(0, 0), location(1, 1));

        // Assert: only a completed FETCH known range may cover this historical gap.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }

    #[tokio::test]
    async fn conflicting_fetch_object_fails_the_fill_and_latches_the_track() {
        // Arrange: a fill stored the object once
        let cache = Arc::new(TrackCache::new());
        fill(
            &cache,
            stream_object_with_payload(0, 0, Bytes::from_static(b"a")),
        )
        .unwrap();

        // Act: a second fill delivers the same object with another payload
        let result = fill(
            &cache,
            stream_object_with_payload(0, 0, Bytes::from_static(b"b")),
        );

        // Assert: the fill fails and the track is quarantined
        assert!(result.is_err());
        assert!(cache.is_malformed());
    }

    const UPSTREAM_SESSION: SessionId = 1;

    /// Fills a fresh cache for {0,0}..{0,3} from an upstream FETCH answered
    /// with `response`.
    async fn ingest(
        response: Vec<moqt::fetch::Fetch>,
    ) -> (
        Arc<TrackCache>,
        mpsc::Receiver<EgressCommand>,
        Result<(), FillFailure>,
    ) {
        ingest_answered(FetchAnswer::Respond(response)).await
    }

    async fn ingest_answered(
        answer: FetchAnswer,
    ) -> (
        Arc<TrackCache>,
        mpsc::Receiver<EgressCommand>,
        Result<(), FillFailure>,
    ) {
        let session_repo = session_repository_with_session(
            UPSTREAM_SESSION,
            mock_session_answering_fetch(answer),
            VerifiedToken::full_access(),
        )
        .await;
        let cache = Arc::new(TrackCache::new());
        let (egress_sender, egress_receiver) = mpsc::channel(1);
        let start = FetchIngestStart {
            track_key: TrackKey::new("ns", "track"),
            upstream_publisher_session_id: UPSTREAM_SESSION,
            fetch_handle: moqt::fetch::FetchHandle {
                request_id: 7,
                group_order: moqt::object::GroupOrder::Ascending,
                end_of_track: false,
                end_location: location(0, 3),
                max_cache_duration: None,
                remainder: None,
            },
            egress_start: EgressFetchRequest {
                subscriber_session_id: 2,
                request_id: 3,
                cache: cache.clone(),
                start_location: location(0, 0),
                end_location: location(0, 3),
                group_order: moqt::object::GroupOrder::Ascending,
            },
            fill_timeout: DEFAULT_FETCH_FILL_TIMEOUT,
        };
        let result = FetchIngest::ingest_fetch_stream(session_repo, &egress_sender, start).await;
        (cache, egress_receiver, result)
    }

    fn fetched(object_id: u64) -> moqt::fetch::Fetch {
        moqt::fetch::Fetch::Object(stream_object(0, object_id).object)
    }

    #[tokio::test]
    async fn a_complete_upstream_response_covers_its_range() {
        // Act
        let (cache, mut egress_receiver, result) =
            ingest(vec![fetched(0), fetched(2), moqt::fetch::Fetch::End]).await;

        // Assert: the gap at {0,1} is known not to exist
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(0, 3)),
            FetchRangeResolution::Serve {
                end_location: location(0, 3)
            }
        );
        assert!(matches!(
            egress_receiver.try_recv(),
            Ok(EgressCommand::StartFetch(_))
        ));
    }

    #[tokio::test]
    async fn an_unknown_range_in_the_upstream_response_leaves_the_range_uncovered() {
        // Act
        let (cache, mut egress_receiver, result) = ingest(vec![
            fetched(0),
            moqt::fetch::Fetch::UnknownRange {
                last: location(0, 1),
            },
            fetched(2),
            moqt::fetch::Fetch::End,
        ])
        .await;

        // Assert: a later FETCH of the range is not served from the cache but
        // goes upstream again, and this one is not served as complete
        assert_eq!(
            cache.resolve_fetch_range(location(0, 0), location(0, 3)),
            FetchRangeResolution::NotCovered
        );
        assert!(
            matches!(result, Err(FillFailure::UnknownObjects)),
            "{result:?}"
        );
        assert!(egress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_malformed_upstream_response_latches_the_track() {
        // Act
        let (cache, mut egress_receiver, result) = ingest_answered(FetchAnswer::Malformed).await;

        // Assert: the fill fails as for a malformed track, so the upstream FETCH is cancelled
        assert!(result.is_err());
        assert!(cache.is_malformed());
        assert!(egress_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn partial_fetch_ingest_does_not_register_coverage() {
        // Arrange: a FETCH fill wrote an object but has not reached Fetch::End.
        let cache = Arc::new(TrackCache::new());
        fill(&cache, stream_object(0, 0)).unwrap();

        // Act
        let resolution = cache.resolve_fetch_range(location(0, 0), location(0, 1));

        // Assert: only Fetch::End may register the filled range as known.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }
}

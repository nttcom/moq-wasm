use moqt::object::DataStreamResetCode;
use std::{ops::Range, sync::Arc, time::Duration};

use tokio::{sync::mpsc, task::JoinHandle, time::Instant};

use crate::modules::{
    data_plane::{
        cache::{
            cached_object::{CacheStamp, CachedObject},
            known_ranges::{KnownRanges, following},
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
    pub(crate) fill_budget: FillBudget,
}

/// How long a FETCH may wait for the objects the relay fetches upstream
/// for it (draft-18 §10.2.5 FILL_TIMEOUT).
#[derive(Debug, Clone, Copy)]
pub(crate) struct FillBudget {
    /// The FETCH's FILL_TIMEOUT and when it runs out, counted from the
    /// FETCH's arrival. The objects not filled by then are reported as
    /// unknown.
    requested: Option<(Duration, Instant)>,
    /// MOQT_FETCH_FILL_TIMEOUT_SECS: without FILL_TIMEOUT, the most a fill
    /// waits before the objects not filled are reported as unknown.
    relay_limit: Duration,
}

impl FillBudget {
    /// The relay waits at most `relay_limit`, even for a longer FILL_TIMEOUT
    /// (§10.2.5 MAY).
    pub(crate) fn new(requested: Option<Duration>, relay_limit: Duration) -> Self {
        Self {
            requested: requested
                .map(|requested| (requested, Instant::now() + requested.min(relay_limit))),
            relay_limit,
        }
    }

    /// FILL_TIMEOUT 0: the FETCH wants only what is at hand (§10.2.5 MUST NOT
    /// wait).
    pub(crate) fn forbids_waiting(&self) -> bool {
        matches!(self.requested, Some((Duration::ZERO, _)))
    }

    /// When a fill starting now has to end.
    fn fill_deadline(&self) -> Instant {
        match self.requested {
            Some((_, deadline)) => deadline,
            None => Instant::now() + self.relay_limit,
        }
    }
}

/// What an ascending fill has shown known: every position from the start of
/// the range up to `next`, except the ranges the response marked unknown.
struct FillProgress {
    known: Vec<Range<moqt::object::Location>>,
    known_from: moqt::object::Location,
    /// The position after the last object or marker received.
    next: moqt::object::Location,
    unknown_seen: bool,
}

impl FillProgress {
    fn new(start: moqt::object::Location) -> Self {
        Self {
            known: Vec::new(),
            known_from: start,
            next: start,
            unknown_seen: false,
        }
    }

    fn object(&mut self, at: moqt::object::Location) {
        self.next = self.next.max(following(at));
    }

    /// draft-18 §11.4.4.2: the positions from `next` through `last` may hold
    /// objects.
    fn unknown(&mut self, last: moqt::object::Location) {
        self.known.push(self.known_from..self.next);
        self.next = self.next.max(following(last));
        self.known_from = self.next;
        self.unknown_seen = true;
    }

    /// The known ranges once the response ended at `end` (exclusive), or
    /// stopped early when `end` is `None`.
    fn into_known(
        mut self,
        end: Option<moqt::object::Location>,
    ) -> Vec<Range<moqt::object::Location>> {
        self.known.push(self.known_from..end.unwrap_or(self.next));
        self.known
    }
}

/// Strict store-and-forward: delivery starts only after the response reached
/// `Fetch::End` or the fill budget ran out, so first-byte latency equals the
/// upstream transfer time.
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
                let error_code = if cache.is_malformed() {
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

    /// Hands the range to egress once the fill ended: as filled when the
    /// response covered all of it, or reporting the rest as unknown when it
    /// marked objects unknown or the fill budget ran out.
    async fn run_inner(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        start: FetchIngestStart,
    ) -> anyhow::Result<()> {
        let cache = start.egress_start.cache.clone();
        let start_eviction_generation = cache.eviction_generation();
        let mut progress = FillProgress::new(start.egress_start.start_location);
        let filled = tokio::time::timeout_at(
            start.fill_budget.fill_deadline(),
            Self::ingest_fetch_stream(session_repo.clone(), &start, &cache, &mut progress),
        )
        .await;
        let end = match filled {
            Ok(result) => {
                result?;
                Some(KnownRanges::exclusive_end(start.egress_start.end_location))
            }
            Err(_) => {
                tracing::info!(
                    request_id = start.egress_start.request_id,
                    "fill timeout ran out; serving the fetch with what arrived"
                );
                Self::cancel_upstream_fetch(
                    session_repo,
                    start.upstream_publisher_session_id,
                    &start.fetch_handle,
                )
                .await;
                None
            }
        };
        if cache.eviction_generation() != start_eviction_generation {
            tracing::warn!(
                request_id = start.egress_start.request_id,
                "fetch fill crossed cache eviction; resetting downstream fetch"
            );
            anyhow::bail!("fetch fill crossed cache eviction");
        }
        let mut egress_start = start.egress_start;
        egress_start.report_unknown = end.is_none() || progress.unknown_seen;
        for known in progress.into_known(end) {
            cache.insert_known_until(known.start, known.end);
        }
        egress_sender
            .send(EgressCommand::StartFetch(egress_start))
            .await?;
        Ok(())
    }

    /// Fills the cache until the response ends (FIN), the fill fails or it
    /// is dropped when its budget runs out.
    async fn ingest_fetch_stream(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        start: &FetchIngestStart,
        cache: &TrackCache,
        progress: &mut FillProgress,
    ) -> anyhow::Result<()> {
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
        let stamp = CacheStamp {
            max_cache_duration: start
                .fetch_handle
                .max_cache_duration
                .map(Duration::from_millis),
            persist,
        };

        loop {
            let received = tokio::select! {
                received = receiver.receive() => received?,
                _ = cache.malformed_track_detected() => {
                    anyhow::bail!("malformed track detected while awaiting fetch data");
                }
            };
            match received {
                moqt::fetch::Fetch::Object(object) => {
                    progress.object(object.location);
                    Self::append_fetch_object(cache, object, stamp)?;
                }
                moqt::fetch::Fetch::UnknownRange { last } => progress.unknown(last),
                moqt::fetch::Fetch::End => return Ok(()),
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
        data_plane::cache::track_cache::{FetchRangeResolution, PlannedFetch},
        session::session_event::EventKind,
        test_support::{
            mock_session::{
                FetchAnswer, RecordedControlMessages, recorded_session_answering_fetch,
                session_repository_with_session, session_repository_with_upstream_session,
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
                report_unknown: false,
            },
            fill_budget: FillBudget::new(None, DEFAULT_FETCH_FILL_TIMEOUT),
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

    struct Ingested {
        cache: Arc<TrackCache>,
        egress_receiver: mpsc::Receiver<EgressCommand>,
        result: anyhow::Result<()>,
        recorded: RecordedControlMessages,
    }

    impl Ingested {
        /// The egress request the fill handed over, if any.
        fn handed_over(&mut self) -> Option<EgressFetchRequest> {
            match self.egress_receiver.try_recv() {
                Ok(EgressCommand::StartFetch(request)) => Some(request),
                _ => None,
            }
        }

        fn served(&self) -> Vec<Result<(u64, u64), (u64, u64)>> {
            self.cache
                .plan_fetch(
                    location(0, 0),
                    location(0, 3),
                    moqt::object::GroupOrder::Ascending,
                )
                .into_iter()
                .map(|step| match step {
                    PlannedFetch::Object(object) => Ok((
                        object.object.location.group_id,
                        object.object.location.object_id,
                    )),
                    PlannedFetch::Unknown { last } => Err((last.group_id, last.object_id)),
                })
                .collect()
        }
    }

    /// Fills a fresh cache for {0,0}..{0,3} from an upstream FETCH answered
    /// with `response`, which then pends.
    async fn ingest(response: Vec<moqt::fetch::Fetch>, fill_budget: FillBudget) -> Ingested {
        let (session, recorded) = recorded_session_answering_fetch(FetchAnswer::Respond(response));
        let session_repo = session_repository_with_session(
            UPSTREAM_SESSION,
            session,
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
                report_unknown: false,
            },
            fill_budget,
        };
        let result = FetchIngest::run_inner(session_repo, &egress_sender, start).await;
        Ingested {
            cache,
            egress_receiver,
            result,
            recorded,
        }
    }

    fn fetched(object_id: u64) -> moqt::fetch::Fetch {
        moqt::fetch::Fetch::Object(stream_object(0, object_id).object)
    }

    fn unlimited() -> FillBudget {
        FillBudget::new(None, DEFAULT_FETCH_FILL_TIMEOUT)
    }

    #[tokio::test]
    async fn a_complete_upstream_response_covers_its_range() {
        // Act
        let mut ingested = ingest(
            vec![fetched(0), fetched(2), moqt::fetch::Fetch::End],
            unlimited(),
        )
        .await;

        // Assert: the gap at {0,1} is known not to exist
        assert!(ingested.result.is_ok(), "{:?}", ingested.result);
        assert_eq!(
            ingested
                .cache
                .resolve_fetch_range(location(0, 0), location(0, 3)),
            FetchRangeResolution::Serve {
                end_location: location(0, 3)
            }
        );
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| !request.report_unknown)
        );
    }

    #[tokio::test]
    async fn an_unknown_range_in_the_upstream_response_is_served_with_the_known_objects() {
        // Act
        let mut ingested = ingest(
            vec![
                fetched(0),
                moqt::fetch::Fetch::UnknownRange {
                    last: location(0, 1),
                },
                fetched(2),
                moqt::fetch::Fetch::End,
            ],
            unlimited(),
        )
        .await;

        // Assert: draft-18 §10.12; a later FETCH of the range goes upstream
        // again, as the cache does not cover it
        assert!(ingested.result.is_ok(), "{:?}", ingested.result);
        assert_eq!(
            ingested
                .cache
                .resolve_fetch_range(location(0, 0), location(0, 3)),
            FetchRangeResolution::NotCovered
        );
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| request.report_unknown)
        );
        assert_eq!(ingested.served(), vec![Ok((0, 0)), Err((0, 1)), Ok((0, 2))]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_fill_timeout_that_runs_out_serves_what_arrived_and_cancels_the_upstream_fetch() {
        // Act: object {0,0} arrives, then the upstream stalls
        let mut ingested = ingest(
            vec![fetched(0)],
            FillBudget::new(Some(Duration::from_millis(50)), DEFAULT_FETCH_FILL_TIMEOUT),
        )
        .await;

        // Assert: draft-18 §10.2.5, the rest is reported as unknown
        assert!(ingested.result.is_ok(), "{:?}", ingested.result);
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| request.report_unknown)
        );
        assert_eq!(ingested.served(), vec![Ok((0, 0)), Err((0, 2))]);
        assert_eq!(
            *ingested
                .recorded
                .fetch_cancelled_request_ids
                .lock()
                .unwrap(),
            vec![7]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_fill_without_fill_timeout_serves_what_arrived_when_the_relay_limit_runs_out() {
        // Act: object {0,0} arrives, then the upstream stalls past the relay limit
        let mut ingested = ingest(
            vec![fetched(0)],
            FillBudget::new(None, Duration::from_millis(50)),
        )
        .await;

        // Assert: draft-18 §10.2.5, without a Fill Timeout the relay waits its
        // own duration before reporting the rest as unknown; a draft-14 sender
        // resets there instead
        assert!(ingested.result.is_ok(), "{:?}", ingested.result);
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| request.report_unknown)
        );
        assert_eq!(ingested.served(), vec![Ok((0, 0)), Err((0, 2))]);
        assert_eq!(
            *ingested
                .recorded
                .fetch_cancelled_request_ids
                .lock()
                .unwrap(),
            vec![7]
        );
    }

    #[test]
    fn a_fill_timeout_above_the_relay_limit_is_cut_to_it() {
        // Arrange
        let now = Instant::now();

        // Act
        let budget = FillBudget::new(Some(Duration::from_secs(600)), Duration::from_secs(20));

        // Assert: draft-18 §10.2.5 MAY use a shorter timeout
        assert!(budget.fill_deadline() <= now + Duration::from_secs(21));
        assert!(!budget.forbids_waiting());
        assert!(FillBudget::new(Some(Duration::ZERO), Duration::from_secs(20)).forbids_waiting());
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

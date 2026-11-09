use moqt::object::DataStreamResetCode;
use std::{ops::Range, sync::Arc, time::Duration};

use tokio::{task::JoinHandle, time::Instant};
use tracing::{Instrument, Span};

use crate::modules::{
    data_plane::{
        cache::{
            cached_object::{CacheStamp, CachedObject},
            known_ranges::{KnownRanges, following},
            track_cache::TrackCache,
        },
        egress::fetch_delivery::{EgressFetchRequest, FetchDeliveryTask, FetchWait},
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
    /// The FETCH's FILL_TIMEOUT.
    requested: Option<Duration>,
    /// When the wait ends, counted from the FETCH's arrival: FILL_TIMEOUT,
    /// at most MOQT_FETCH_FILL_TIMEOUT_SECS, which also applies without
    /// FILL_TIMEOUT. The objects not filled by then are reported as unknown.
    deadline: Instant,
}

impl FillBudget {
    /// The relay waits at most `relay_limit`, even for a longer FILL_TIMEOUT
    /// (§10.2.5 MAY).
    pub(crate) fn new(requested: Option<Duration>, relay_limit: Duration) -> Self {
        let wait = requested.map_or(relay_limit, |requested| requested.min(relay_limit));
        Self {
            requested,
            deadline: Instant::now() + wait,
        }
    }

    /// FILL_TIMEOUT 0: the FETCH wants only what is at hand (§10.2.5 MUST NOT
    /// wait).
    pub(crate) fn forbids_waiting(&self) -> bool {
        self.requested == Some(Duration::ZERO)
    }

    /// How long a FETCH served from the cache waits for the objects live
    /// ingest has yet to deliver. Without FILL_TIMEOUT the relay limit
    /// applies only when the response can report the rest as unknown
    /// (`marks_unknown_ranges`); otherwise stopping would reset it, so the
    /// delivery waits for them.
    pub(crate) fn delivery_wait(&self, marks_unknown_ranges: bool) -> FetchWait {
        match self.requested {
            Some(Duration::ZERO) => FetchWait::Never,
            Some(_) => FetchWait::Until(self.deadline),
            None if marks_unknown_ranges => FetchWait::Until(self.deadline),
            None => FetchWait::Unlimited,
        }
    }

    /// When a fill has to end.
    fn fill_deadline(&self) -> Instant {
        self.deadline
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
        start: FetchIngestStart,
    ) -> Self {
        let downstream_subscriber_session_id = start.egress_start.subscriber_session_id;
        let request_id = start.egress_start.request_id;
        let upstream_publisher_session_id = start.upstream_publisher_session_id;
        let upstream_fetch = start.fetch_handle.clone();
        let track_key = start.track_key.clone();
        let cache = start.egress_start.cache.clone();
        let join_handle = tokio::spawn(
            async move {
                let filled = match Self::run_inner(session_repo.clone(), start).await {
                    Ok(egress_start) => Self::deliver(&session_repo, egress_start).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = filled {
                    // These failures are request-scoped and expected (timeout, upstream reset).
                    // Log the chain without anyhow's backtrace.
                    tracing::error!(error = %format!("{error:#}"), "fetch ingest failed");
                    let error_code = if cache.is_malformed() {
                        let publisher_mailbox_sender = session_repo
                            .lock()
                            .await
                            .mailbox(upstream_publisher_session_id);
                        if let Some(publisher_mailbox_sender) = publisher_mailbox_sender {
                            let _ = publisher_mailbox_sender
                                .send(SessionEvent::MalformedTrackDetected(track_key));
                        }
                        Self::cancel_upstream_fetch(
                            session_repo.clone(),
                            upstream_publisher_session_id,
                            &upstream_fetch,
                        )
                        .await;
                        DataStreamResetCode::MalformedTrack
                    } else {
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
            }
            .instrument(Span::current()),
        );
        Self {
            _join_handle: join_handle,
        }
    }

    async fn deliver(
        session_repo: &tokio::sync::Mutex<SessionRepository>,
        egress_start: EgressFetchRequest,
    ) -> anyhow::Result<()> {
        let publisher = session_repo
            .lock()
            .await
            .publisher(egress_start.subscriber_session_id)
            .ok_or_else(|| anyhow::anyhow!("downstream subscriber session not found"))?;
        let _delivery = FetchDeliveryTask::run(publisher, egress_start);
        Ok(())
    }

    /// Fills the cache and returns the range to hand to egress once the fill
    /// ended: as filled when the response covered all of it, or reporting the
    /// rest as unknown when it marked objects unknown or the fill budget ran
    /// out.
    async fn run_inner(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        start: FetchIngestStart,
    ) -> anyhow::Result<EgressFetchRequest> {
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
                    session_repo.clone(),
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
        if end.is_none() || progress.unknown_seen {
            egress_start.wait = FetchWait::Never;
        }
        for known in progress.into_known(end) {
            cache.insert_known_until(known.start, known.end);
        }
        Ok(egress_start)
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
            status_may_change: !receiver.compares_duplicate_status(),
        };

        loop {
            let received = tokio::select! {
                received = receiver.receive() => received.inspect_err(|error| {
                    if error.is::<moqt::object::MalformedTrackError>() {
                        cache.mark_malformed();
                    }
                })?,
                _ = cache.malformed_track_detected() => {
                    anyhow::bail!("malformed track detected while awaiting fetch data");
                }
            };
            match received {
                moqt::fetch::Fetch::Object(object) => {
                    progress.object(object.location);
                    let mut object = stamp.apply(CachedObject::from_object(object));
                    object.forwarding_inferred = !receiver.carries_forwarding_preference();
                    Self::append_fetch_object(cache, object)?;
                }
                moqt::fetch::Fetch::UnknownRange { last } => progress.unknown(last),
                moqt::fetch::Fetch::End => return Ok(()),
            }
        }
    }

    fn append_fetch_object(cache: &TrackCache, object: CachedObject) -> anyhow::Result<()> {
        let (group_id, object_id) = (
            object.object.location.group_id,
            object.object.location.object_id,
        );
        cache.insert(object).map_err(|_| {
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
            tracing::debug!(upstream_request_id, "sent upstream FETCH_CANCEL");
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
        data_plane::cache::{subgroup_key::SubgroupKey, track_cache::FetchRangeResolution},
        session::session_event::SessionEvent,
        test_support::{
            mock_session::{
                FetchAnswer, RecordedControlMessages, recorded_session_answering_fetch,
                session_repository_with_session,
                session_repository_with_upstream_session_and_mailbox,
            },
            relay_harness::fixtures::{
                cached_object::{
                    datagram_object, status_object, stream_object, stream_object_in_subgroup,
                    stream_object_with_payload,
                },
                location, planned_steps,
            },
        },
    };
    use bytes::Bytes;
    use moqt::object::{MalformedTrackError, ObjectStatus};

    fn fill(cache: &TrackCache, object: CachedObject) -> anyhow::Result<()> {
        FetchIngest::append_fetch_object(cache, object)
    }

    #[tokio::test]
    async fn malformed_track_cancels_upstream_fetch_while_awaiting_data() {
        // Arrange: the track is latched malformed by conflicting duplicate objects.
        const UPSTREAM_SESSION: SessionId = 1;
        const DOWNSTREAM_SESSION: SessionId = 2; // not registered: downstream reset is skipped
        const UPSTREAM_FETCH_REQUEST_ID: u64 = 7;
        let (session_repo, recorded, mut publisher_mailbox_receiver) =
            session_repository_with_upstream_session_and_mailbox(UPSTREAM_SESSION);

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
                wait: FetchWait::Unlimited,
            },
            fill_budget: FillBudget::new(None, DEFAULT_FETCH_FILL_TIMEOUT),
        };

        // Act: the receiver pends forever, so only the malformed latch can
        // make the ingest bail.
        let _ingest = FetchIngest::run(session_repo, start);

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
        // Assert: the detection reaches the publisher session for upstream subscription teardown.
        let event = publisher_mailbox_receiver
            .try_recv()
            .expect("malformed detection should be reported to the publisher session");
        assert!(matches!(
            event,
            SessionEvent::MalformedTrackDetected(track_key) if track_key == TrackKey::new("ns", "track")
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
        let resolution = cache.resolve_fetch_range(location(0, 0), location(1, 1), None);

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
        result: anyhow::Result<EgressFetchRequest>,
        recorded: RecordedControlMessages,
    }

    impl Ingested {
        fn handed_over(&mut self) -> Option<EgressFetchRequest> {
            std::mem::replace(&mut self.result, Err(anyhow::anyhow!("taken"))).ok()
        }

        fn served(&self) -> Vec<Result<(u64, u64), (u64, u64)>> {
            planned_steps(self.cache.plan_fetch(
                location(0, 0),
                location(0, 3),
                moqt::object::GroupOrder::Ascending,
            ))
        }
    }

    /// Fills a fresh cache for {0,0}..{0,3} from an upstream FETCH answered
    /// with `response`, which then pends.
    async fn ingest(response: Vec<moqt::fetch::Fetch>, fill_budget: FillBudget) -> Ingested {
        ingest_into(
            Arc::new(TrackCache::new()),
            FetchAnswer::Respond(response),
            fill_budget,
        )
        .await
    }

    async fn ingest_into(
        cache: Arc<TrackCache>,
        answer: FetchAnswer,
        fill_budget: FillBudget,
    ) -> Ingested {
        let (session, recorded) = recorded_session_answering_fetch(answer);
        let session_repo = session_repository_with_session(
            UPSTREAM_SESSION,
            session,
            VerifiedToken::full_access(),
        )
        .await;
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
                wait: FetchWait::Unlimited,
            },
            fill_budget,
        };
        let result = FetchIngest::run_inner(session_repo, start).await;
        Ingested {
            cache,
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
        assert!(
            ingested.result.is_ok(),
            "{:?}",
            ingested.result.as_ref().err()
        );
        assert_eq!(
            ingested
                .cache
                .resolve_fetch_range(location(0, 0), location(0, 3), None),
            FetchRangeResolution::Serve {
                end_location: location(0, 3)
            }
        );
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| request.wait == FetchWait::Unlimited)
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
        assert!(
            ingested.result.is_ok(),
            "{:?}",
            ingested.result.as_ref().err()
        );
        assert_eq!(
            ingested
                .cache
                .resolve_fetch_range(location(0, 0), location(0, 3), None),
            FetchRangeResolution::NotCovered
        );
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| request.wait == FetchWait::Never)
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

        // Assert: draft-18 §10.2.5, the plan reports the rest as unknown; as it
        // ends in group 0 of the last object, the sender resets there (§10.12)
        assert!(
            ingested.result.is_ok(),
            "{:?}",
            ingested.result.as_ref().err()
        );
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| request.wait == FetchWait::Never)
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
        // own duration before reporting the rest as unknown, which the sender
        // turns into a reset here (§10.12)
        assert!(
            ingested.result.is_ok(),
            "{:?}",
            ingested.result.as_ref().err()
        );
        assert!(
            ingested
                .handed_over()
                .is_some_and(|request| request.wait == FetchWait::Never)
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

    /// A cache holding the live datagram {0,1}, filled from a response with
    /// that object in subgroup 1.
    async fn fill_over_a_datagram(
        respond: fn(Vec<moqt::fetch::Fetch>) -> FetchAnswer,
    ) -> Arc<TrackCache> {
        let cache = Arc::new(TrackCache::new());
        let _ = cache
            .open_subgroup(SubgroupKey::Datagram { group_id: 0 })
            .insert(datagram_object(0, 1));
        let copy = moqt::fetch::Fetch::Object(stream_object_in_subgroup(0, 1, 1).object);
        let _ = ingest_into(
            cache.clone(),
            respond(vec![copy, moqt::fetch::Fetch::End]),
            unlimited(),
        )
        .await;
        cache
    }

    #[tokio::test]
    async fn a_draft14_copy_of_a_datagram_in_the_subgroup_of_its_object_id_matches_it() {
        // Act
        let cache = fill_over_a_datagram(FetchAnswer::Respond).await;
        // Assert
        assert!(!cache.is_malformed());
    }

    #[tokio::test]
    async fn a_draft18_copy_of_a_datagram_in_a_subgroup_latches_the_track() {
        // Act
        let cache = fill_over_a_datagram(FetchAnswer::RespondWithPreference).await;
        // Assert
        assert!(cache.is_malformed());
    }

    /// A cache filled from a response that holds {0,0} as an empty Normal
    /// object, then as End of Group.
    async fn fill_with_a_status_only_duplicate(
        respond: fn(Vec<moqt::fetch::Fetch>) -> FetchAnswer,
    ) -> Arc<TrackCache> {
        let copy = |status| moqt::fetch::Fetch::Object(status_object(0, 0, status).object);
        let ingested = ingest_into(
            Arc::new(TrackCache::new()),
            respond(vec![
                copy(ObjectStatus::Normal),
                copy(ObjectStatus::EndOfGroup),
                moqt::fetch::Fetch::End,
            ]),
            unlimited(),
        )
        .await;
        ingested.cache
    }

    #[tokio::test]
    async fn a_draft14_fetch_copy_that_changes_only_its_status_latches_the_track() {
        // Act
        let cache = fill_with_a_status_only_duplicate(FetchAnswer::Respond).await;
        // Assert: draft-14 §8.1, a move between Normal and End of Group is malformed
        assert!(cache.is_malformed());
    }

    #[tokio::test]
    async fn a_draft18_fetch_copy_that_changes_only_its_status_does_not_latch_the_track() {
        // Act
        let cache = fill_with_a_status_only_duplicate(FetchAnswer::RespondWithPreference).await;
        // Assert: draft-18 §9.1 compares only the forwarding preference,
        // subgroup, priority and payload
        assert!(!cache.is_malformed());
        assert_eq!(cache.occupancy().objects, 1);
    }

    #[tokio::test]
    async fn a_malformed_upstream_response_latches_the_track() {
        // Act
        let mut ingested = ingest_into(
            Arc::new(TrackCache::new()),
            FetchAnswer::Malformed(MalformedTrackError::ConflictingDuplicate {
                location: location(0, 0),
            }),
            unlimited(),
        )
        .await;

        // Assert: the fill fails as for a malformed track, so the upstream FETCH is cancelled
        assert!(ingested.result.is_err());
        assert!(ingested.cache.is_malformed());
        assert!(ingested.handed_over().is_none());
    }

    #[tokio::test]
    async fn an_upstream_fetch_object_with_malformed_immutable_properties_latches_the_track() {
        // Act: moqt withholds the object and reports why
        let mut ingested = ingest_into(
            Arc::new(TrackCache::new()),
            FetchAnswer::Malformed(MalformedTrackError::MalformedImmutableProperties {
                location: location(0, 0),
            }),
            unlimited(),
        )
        .await;

        // Assert: draft-14 §11.2 / draft-18 §12.7 make the track malformed, so
        // the fill fails and nothing reaches the cache
        assert!(ingested.result.is_err());
        assert!(ingested.cache.is_malformed());
        assert_eq!(ingested.cache.occupancy().objects, 0);
        assert!(ingested.handed_over().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn the_relay_limit_of_an_upstream_fill_counts_from_the_fetch_arrival() {
        // Arrange
        let arrival = Instant::now();
        let budget = FillBudget::new(None, Duration::from_secs(20));
        tokio::time::advance(Duration::from_secs(5)).await;

        // Act: the upstream FETCH_OK arrives and the fill starts
        let deadline = budget.fill_deadline();

        // Assert: the same limit a cache-served FETCH waits for (delivery_wait)
        assert_eq!(deadline, arrival + Duration::from_secs(20));
        assert_eq!(budget.delivery_wait(true), FetchWait::Until(deadline));
    }

    #[tokio::test(start_paused = true)]
    async fn a_cache_served_fetch_waits_as_its_fill_timeout_and_draft_allow() {
        // Arrange
        let arrival = Instant::now();
        let relay_limit = Duration::from_secs(20);

        // Act
        let zero = FillBudget::new(Some(Duration::ZERO), relay_limit).delivery_wait(true);
        let bounded =
            FillBudget::new(Some(Duration::from_millis(500)), relay_limit).delivery_wait(false);
        let draft18 = FillBudget::new(None, relay_limit).delivery_wait(true);
        let draft14 = FillBudget::new(None, relay_limit).delivery_wait(false);

        // Assert: draft-18 §10.2.5; draft-14 §9.16 cannot report objects
        // unknown, so its delivery waits for them
        assert_eq!(zero, FetchWait::Never);
        assert_eq!(
            bounded,
            FetchWait::Until(arrival + Duration::from_millis(500))
        );
        assert_eq!(draft18, FetchWait::Until(arrival + relay_limit));
        assert_eq!(draft14, FetchWait::Unlimited);
    }

    #[tokio::test]
    async fn partial_fetch_ingest_does_not_register_coverage() {
        // Arrange: a FETCH fill wrote an object but has not reached Fetch::End.
        let cache = Arc::new(TrackCache::new());
        fill(&cache, stream_object(0, 0)).unwrap();

        // Act
        let resolution = cache.resolve_fetch_range(location(0, 0), location(0, 1), None);

        // Assert: only Fetch::End may register the filled range as known.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }
}

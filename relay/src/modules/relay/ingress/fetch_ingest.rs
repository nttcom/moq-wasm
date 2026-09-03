use std::sync::Arc;

use tokio::{sync::mpsc, task::JoinHandle};

use crate::modules::{
    enums::FetchErrorCode,
    relay::{
        cache::{
            cached_object::CachedObject,
            duration::duration_from_env,
            track_cache::{InsertOrigin, TrackCache},
        },
        egress::coordinator::{EgressCommand, EgressFetchRequest},
    },
    session_event::SessionEvent,
    session_repository::SessionRepository,
    types::{SessionId, TrackKey},
};

const DATA_STREAM_INTERNAL_ERROR: u64 = 0x0;
const DEFAULT_FETCH_FILL_TIMEOUT_SECS: u64 = 20;

pub(crate) struct FetchIngestStart {
    pub(crate) track_key: TrackKey,
    pub(crate) upstream_publisher_session_id: SessionId,
    pub(crate) downstream_subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) fetch_handle: moqt::FetchHandle,
    pub(crate) cache: Arc<TrackCache>,
    pub(crate) requested_start: moqt::Location,
    pub(crate) requested_end: moqt::Location,
    pub(crate) egress_start: EgressFetchRequest,
}

/// Ingests one upstream FETCH response into the track cache, then hands
/// delivery to egress.
///
/// v1 limitation: strict store-and-forward. Delivery starts only after the
/// whole response reached `Fetch::End`, so first-byte latency equals the
/// upstream transfer time and fills longer than MOQT_FETCH_FILL_TIMEOUT_SECS
/// fail. Streaming delivery (serving while filling, bounded by the knowledge
/// frontier) is planned as a follow-up.
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
        let downstream_subscriber_session_id = start.downstream_subscriber_session_id;
        let request_id = start.request_id;
        let upstream_publisher_session_id = start.upstream_publisher_session_id;
        let upstream_request_id = start.fetch_handle.request_id;
        let track_key = start.track_key.clone();
        let cache = start.cache.clone();
        let join_handle = tokio::spawn(async move {
            if let Err(error) = Self::run_inner(session_repo.clone(), &egress_sender, start).await {
                // Expected request-scoped failures (timeout, upstream reset):
                // log the chain without anyhow's captured backtrace.
                tracing::error!(error = %format!("{error:#}"), "fetch ingest failed");
                let error_code = if cache.is_malformed() {
                    let _ = session_event_sender.send(SessionEvent::malformed_track_detected(
                        upstream_publisher_session_id,
                        track_key,
                    ));
                    Self::cancel_upstream_fetch(
                        session_repo.clone(),
                        upstream_publisher_session_id,
                        upstream_request_id,
                    )
                    .await;
                    FetchErrorCode::MalformedTrack as u64
                } else {
                    DATA_STREAM_INTERNAL_ERROR
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
    ) -> anyhow::Result<()> {
        let timeout = Self::timeout();
        let result = tokio::time::timeout(
            timeout,
            Self::ingest_fetch_stream(session_repo.clone(), egress_sender, start),
        )
        .await;

        match result {
            Ok(result) => result,
            Err(_) => {
                anyhow::bail!("fetch ingest timed out after {:?}", timeout);
            }
        }
    }

    async fn ingest_fetch_stream(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        egress_sender: &tokio::sync::mpsc::Sender<EgressCommand>,
        start: FetchIngestStart,
    ) -> anyhow::Result<()> {
        let mut subscriber = {
            let session_repo = session_repo.lock().await;
            session_repo
                .subscriber(start.upstream_publisher_session_id)
                .ok_or_else(|| anyhow::anyhow!("upstream publisher session not found"))?
        };
        let mut receiver = subscriber
            .create_fetch_receiver(&start.fetch_handle)
            .await?;
        let start_eviction_generation = start.cache.eviction_generation();

        loop {
            let received = tokio::select! {
                received = receiver.receive() => received?,
                _ = start.cache.malformed_track_detected() => {
                    anyhow::bail!("malformed track detected while awaiting fetch data");
                }
            };
            match received {
                moqt::Fetch::Header(_) => {}
                moqt::Fetch::Object(object) => {
                    Self::append_fetch_object(&start.cache, object)?;
                }
                moqt::Fetch::End => {
                    if start.cache.eviction_generation() != start_eviction_generation {
                        tracing::warn!(
                            request_id = start.request_id,
                            "fetch fill crossed cache eviction; resetting downstream fetch"
                        );
                        anyhow::bail!("fetch fill crossed cache eviction");
                    }
                    start
                        .cache
                        .insert_fetch_known_range(start.requested_start, start.requested_end);
                    egress_sender
                        .send(EgressCommand::StartFetch(start.egress_start))
                        .await?;
                    return Ok(());
                }
            }
        }
    }

    fn append_fetch_object(
        cache: &TrackCache,
        object: moqt::FetchObjectField,
    ) -> anyhow::Result<()> {
        let (group_id, object_id) = (object.group_id, object.object_id);
        cache
            .insert(CachedObject::from_fetch_object(object), InsertOrigin::Fill)
            .map(|_| ())
            .map_err(|_| {
                anyhow::anyhow!(
                    "malformed track detected during fetch fill (group {group_id}, object {object_id})"
                )
            })
    }

    fn timeout() -> std::time::Duration {
        duration_from_env(
            "MOQT_FETCH_FILL_TIMEOUT_SECS",
            DEFAULT_FETCH_FILL_TIMEOUT_SECS,
        )
    }

    async fn cancel_upstream_fetch(
        session_repo: Arc<tokio::sync::Mutex<SessionRepository>>,
        upstream_publisher_session_id: SessionId,
        upstream_request_id: u64,
    ) {
        let subscriber = {
            let session_repo = session_repo.lock().await;
            session_repo.subscriber(upstream_publisher_session_id)
        };
        let Some(subscriber) = subscriber else {
            return;
        };
        if let Err(error) = subscriber.send_fetch_cancel(upstream_request_id).await {
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
        error_code: u64,
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
    use crate::modules::{
        core::mocks::session_repository_with_upstream_session,
        relay::cache::track_cache::FetchRangeResolution, session_event::EventKind,
    };
    use bytes::Bytes;
    use moqt::{ExtensionHeaders, FetchObject, FetchObjectField, ObjectStatus};
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn malformed_track_cancels_upstream_fetch_while_awaiting_data() {
        // Arrange: the track is latched malformed by conflicting duplicate objects.
        const UPSTREAM_SESSION: SessionId = 1;
        const DOWNSTREAM_SESSION: SessionId = 2; // not registered: downstream reset is skipped
        const UPSTREAM_FETCH_REQUEST_ID: u64 = 7;
        let (session_repo, recorded) =
            session_repository_with_upstream_session(UPSTREAM_SESSION).await;

        let cache = Arc::new(TrackCache::new());
        FetchIngest::append_fetch_object(
            &cache,
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders::default(),
                FetchObject::Payload(Bytes::from_static(b"a")),
            ),
        )
        .unwrap();
        let _ = FetchIngest::append_fetch_object(
            &cache,
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders::default(),
                FetchObject::Payload(Bytes::from_static(b"b")),
            ),
        );
        assert!(cache.is_malformed());

        let (egress_sender, _egress_receiver) = mpsc::channel(1);
        let (session_event_sender, mut session_event_receiver) = mpsc::unbounded_channel();
        let location = |group_id, object_id| moqt::Location {
            group_id,
            object_id,
        };
        let start = FetchIngestStart {
            track_key: TrackKey::new("ns", "track"),
            upstream_publisher_session_id: UPSTREAM_SESSION,
            downstream_subscriber_session_id: DOWNSTREAM_SESSION,
            request_id: 3,
            fetch_handle: moqt::FetchHandle {
                request_id: UPSTREAM_FETCH_REQUEST_ID,
                group_order: moqt::GroupOrder::Ascending,
                end_of_track: false,
                end_location: location(0, 1),
            },
            cache: cache.clone(),
            requested_start: location(0, 0),
            requested_end: location(0, 1),
            egress_start: EgressFetchRequest {
                subscriber_session_id: DOWNSTREAM_SESSION,
                request_id: 3,
                cache: cache.clone(),
                start_location: location(0, 0),
                end_location: location(0, 1),
                group_order: moqt::GroupOrder::Ascending,
            },
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
    async fn fetch_status_object_does_not_close_live_subgroup() {
        // Arrange: a FETCH response can contain EndOfGroup status, but that
        // must not close the shared live GroupCache.
        let cache = Arc::new(TrackCache::new());
        FetchIngest::append_fetch_object(
            &cache,
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders::default(),
                FetchObject::Status(ObjectStatus::EndOfGroup),
            ),
        )
        .unwrap();
        FetchIngest::append_fetch_object(
            &cache,
            FetchObjectField::new(
                1,
                0,
                0,
                0,
                ExtensionHeaders::default(),
                FetchObject::Payload(Bytes::new()),
            ),
        )
        .unwrap();

        // Act
        let resolution = cache.resolve_fetch_range(
            moqt::Location {
                group_id: 0,
                object_id: 0,
            },
            moqt::Location {
                group_id: 1,
                object_id: 1,
            },
        );

        // Assert: only a completed FETCH known range may cover this historical gap.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }

    #[tokio::test]
    async fn conflicting_fetch_object_fails_the_fill_and_latches_the_track() {
        // Arrange: a fill stored the object once
        let cache = Arc::new(TrackCache::new());
        FetchIngest::append_fetch_object(
            &cache,
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders::default(),
                FetchObject::Payload(Bytes::from_static(b"a")),
            ),
        )
        .unwrap();

        // Act: a second fill delivers the same object with another payload
        let result = FetchIngest::append_fetch_object(
            &cache,
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders::default(),
                FetchObject::Payload(Bytes::from_static(b"b")),
            ),
        );

        // Assert: the fill fails and the track is quarantined
        assert!(result.is_err());
        assert!(cache.is_malformed());
    }

    #[tokio::test]
    async fn partial_fetch_ingest_does_not_register_coverage() {
        // Arrange: a FETCH fill wrote an object but has not reached Fetch::End.
        let cache = Arc::new(TrackCache::new());
        FetchIngest::append_fetch_object(
            &cache,
            FetchObjectField::new(
                0,
                0,
                0,
                0,
                ExtensionHeaders::default(),
                FetchObject::Payload(Bytes::new()),
            ),
        )
        .unwrap();

        // Act
        let resolution = cache.resolve_fetch_range(
            moqt::Location {
                group_id: 0,
                object_id: 0,
            },
            moqt::Location {
                group_id: 0,
                object_id: 1,
            },
        );

        // Assert: only Fetch::End may register the filled range as known.
        assert_eq!(resolution, FetchRangeResolution::NotCovered);
    }
}

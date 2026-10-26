use std::sync::Arc;

use tokio::{
    sync::mpsc::{Sender, UnboundedSender},
    task::JoinHandle,
};
use tracing::Instrument;

use super::{Fetch, FetchTarget, NoUpstreamFetch};
use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder, sequences::session_peer,
        upstream_publisher_resolver::UpstreamPublisherResolver,
    },
    data_plane::{
        cache::store::TrackCacheStore,
        egress::coordinator::{EgressCommand, EgressFetchRequest, FetchWait},
        ingress::fetch_ingest::{FetchIngest, FetchIngestStart, FillBudget},
    },
    domain::{pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId},
    session::{handler::fetch::FetchHandler, session_event::SessionEvent},
};

pub(super) struct UpstreamFetchStart {
    pub(super) session_id: SessionId,
    pub(super) handler: Box<dyn FetchHandler>,
    pub(super) target: FetchTarget,
    pub(super) table: Arc<InMemoryLocalPubSubDirectory>,
    pub(super) forwarder: ControlMessageForwarder,
    pub(super) upstream_publisher_resolver: Arc<UpstreamPublisherResolver>,
    pub(super) cache_store: Arc<TrackCacheStore>,
    pub(super) egress_sender: Sender<EgressCommand>,
    pub(super) session_event_sender: UnboundedSender<SessionEvent>,
    pub(super) fill_budget: FillBudget,
}

/// Off the session worker so a publisher that never answers FETCH does not stall the session.
pub(super) struct UpstreamFetchTask {
    _join_handle: JoinHandle<()>,
}

impl UpstreamFetchTask {
    pub(super) fn run(start: UpstreamFetchStart) -> Self {
        let join_handle = tokio::spawn(Self::forward(start).instrument(tracing::Span::current()));
        Self {
            _join_handle: join_handle,
        }
    }

    async fn forward(start: UpstreamFetchStart) {
        let requester = session_peer(start.session_id, &start.forwarder).await;
        let prepared = match Fetch::create_upstream_fetch(
            &start.table,
            &start.forwarder,
            &start.upstream_publisher_resolver,
            requester,
            start.handler.as_ref(),
            &start.target,
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(NoUpstreamFetch::MalformedTrack(publisher_session_id)) => {
                tracing::warn!(
                    pub_session_id = publisher_session_id,
                    track_key = %start.target.track_key,
                    "upstream FETCH refused the track as malformed; latching it"
                );
                // draft-18 §2.4.2: the latch ends every downstream
                // subscription and FETCH of the track with MALFORMED_TRACK.
                start
                    .cache_store
                    .get_or_create(&start.target.track_key)
                    .mark_malformed();
                let _ = start
                    .session_event_sender
                    .send(SessionEvent::malformed_track_detected(
                        publisher_session_id,
                        start.target.track_key,
                    ));
                return;
            }
            Err(NoUpstreamFetch::Refused) => return,
        };

        if let Err(e) = start
            .handler
            .ok(
                prepared.handle.end_of_track,
                prepared.handle.end_location,
                prepared.handle.track_properties.clone(),
            )
            .await
        {
            tracing::error!(?e, "Failed to send FETCH_OK to downstream");
            return;
        }

        let egress_start = EgressFetchRequest {
            subscriber_session_id: start.session_id,
            request_id: start.handler.request_id(),
            cache: start.cache_store.get_or_create(&start.target.track_key),
            start_location: start.target.start_location,
            end_location: prepared.handle.end_location,
            group_order: start.handler.group_order(),
            wait: FetchWait::Unlimited,
        };
        let _fetch_ingest = FetchIngest::run(
            start.forwarder.repository.clone(),
            start.egress_sender,
            start.session_event_sender,
            FetchIngestStart {
                track_key: start.target.track_key,
                upstream_publisher_session_id: prepared.upstream_publisher_session_id,
                fetch_handle: prepared.handle,
                egress_start,
                fill_budget: start.fill_budget,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use moqt::RequestErrorCode;

    use super::*;
    use crate::{
        config::DEFAULT_FETCH_FILL_TIMEOUT,
        modules::{
            auth::verified_token::VerifiedToken,
            domain::{session_peer::SessionPeer, track_key::TrackKey},
            session::session_event::EventKind,
            test_support::{
                directory_fixtures::local_publisher_resolver,
                mock_session::{
                    FetchAnswer, MockFetchHandler, mock_session_answering_fetch,
                    session_repository_with_sessions,
                },
                relay_harness::fixtures::location,
            },
        },
    };

    #[tokio::test]
    async fn an_upstream_fetch_refused_as_malformed_latches_the_track_and_answers_malformed_track()
    {
        // Arrange: the newest publisher refuses the FETCH as malformed; an
        // older one would answer it
        const MALFORMED_PUBLISHER: SessionId = 5;
        const ANSWERING_PUBLISHER: SessionId = 3;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        for publisher_session_id in [ANSWERING_PUBLISHER, MALFORMED_PUBLISHER] {
            table.register_publish_namespace(
                publisher_session_id,
                "ns".to_string(),
                SessionPeer::Client,
            );
        }
        let repository = session_repository_with_sessions(
            vec![
                (
                    MALFORMED_PUBLISHER,
                    mock_session_answering_fetch(FetchAnswer::Refuse(
                        RequestErrorCode::MalformedTrack,
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
        let handler = MockFetchHandler::default();
        let answered_errors = handler.errors.clone();
        let track_key = TrackKey::new("ns", "track");
        let cache_store = Arc::new(TrackCacheStore::new());
        let (egress_sender, _egress_receiver) = tokio::sync::mpsc::channel(1);
        let (session_event_sender, mut session_event_receiver) =
            tokio::sync::mpsc::unbounded_channel();

        // Act
        let task = UpstreamFetchTask::run(UpstreamFetchStart {
            session_id: 2,
            handler: Box::new(handler),
            target: FetchTarget {
                track_key: track_key.clone(),
                track_namespace: "ns".to_string(),
                track_name: "track".to_string(),
                start_location: location(0, 0),
                end_location: location(1, 0),
            },
            table,
            forwarder: ControlMessageForwarder { repository },
            upstream_publisher_resolver: Arc::new(local_publisher_resolver()),
            cache_store: cache_store.clone(),
            egress_sender,
            session_event_sender,
            fill_budget: FillBudget::new(None, DEFAULT_FETCH_FILL_TIMEOUT),
        });
        tokio::time::timeout(Duration::from_secs(1), task._join_handle)
            .await
            .unwrap()
            .unwrap();

        // Assert: draft-18 §2.4.2, §12.7; the detection ends the track's
        // upstream subscriptions
        assert_eq!(
            *answered_errors.lock().unwrap(),
            vec![RequestErrorCode::MalformedTrack]
        );
        assert!(
            cache_store
                .get(&track_key)
                .is_some_and(|cache| cache.is_malformed())
        );
        let event = session_event_receiver
            .try_recv()
            .expect("the detection should be reported");
        assert_eq!(event.session_id, MALFORMED_PUBLISHER);
        assert!(matches!(
            event.kind,
            EventKind::MalformedTrackDetected(reported) if reported == track_key
        ));
    }
}

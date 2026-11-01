use std::sync::Arc;

use tokio::task::{JoinHandle, JoinSet};
use tracing::{Instrument, Span};

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{IngestRequest, session_peer, start_ingest},
        upstream_publisher_resolver::{UpstreamPublisher, UpstreamPublisherResolver},
    },
    data_plane::cache::store::TrackCacheStore,
    directory::{
        InMemoryLocalPubSubDirectory,
        track::{Producer, ProducerOrigin, ProducerState},
    },
    domain::{
        publish_done_reason::PublishDoneReason, session_id::SessionId, session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::subscription::UpstreamSubscription,
};

pub(crate) struct AnsweredUpstreamSubscribe {
    pub(crate) publisher_session_id: SessionId,
    pub(crate) publisher_peer: SessionPeer,
    pub(crate) subscribed: anyhow::Result<UpstreamSubscription>,
}

pub(crate) type PendingUpstreamSubscribes = JoinSet<Option<AnsweredUpstreamSubscribe>>;

/// Each publisher is dialled and subscribed in its own task, so a relay that
/// cannot be reached or a session that never answers holds back no other.
pub(crate) fn send_upstream_subscribes(
    resolver: &Arc<UpstreamPublisherResolver>,
    forwarder: &ControlMessageForwarder,
    track_key: &TrackKey,
    publishers: Vec<UpstreamPublisher>,
) -> PendingUpstreamSubscribes {
    let mut pending = JoinSet::new();
    for publisher in publishers {
        let resolver = resolver.clone();
        let forwarder = forwarder.clone();
        let track_key = track_key.clone();
        pending.spawn(
            async move {
                let publisher_session_id = resolver.session_of(&publisher).await?;
                let publisher_peer = session_peer(publisher_session_id, &forwarder).await;
                let subscribed = forwarder
                    .subscribe(
                        publisher_session_id,
                        track_key.track_namespace,
                        track_key.track_name,
                    )
                    .await;
                Some(AnsweredUpstreamSubscribe {
                    publisher_session_id,
                    publisher_peer,
                    subscribed,
                })
            }
            .instrument(Span::current()),
        );
    }
    pending
}

pub(crate) async fn next_answered(
    pending: &mut PendingUpstreamSubscribes,
) -> Option<AnsweredUpstreamSubscribe> {
    while let Some(answered) = pending.join_next().await {
        if let Ok(Some(answered)) = answered {
            return Some(answered);
        }
    }
    None
}

pub(crate) fn subscribe_initiated(
    subscription: &UpstreamSubscription,
    publisher_peer: SessionPeer,
) -> ProducerState {
    ProducerState {
        subscription: subscription.clone(),
        expires: subscription.expires(),
        content_exists: subscription.content_exists(),
        origin: ProducerOrigin::Subscribe,
        peer: publisher_peer,
    }
}

/// Ends the registered upstream subscription, and with it its ingest, when
/// the publisher session has left the repository; returns true in that case.
/// Call it after `register_upstream_subscription`: a session leaves the
/// repository before its cleanup reads the directory, so either this check or
/// the cleanup sees the subscription.
pub(crate) async fn end_if_publisher_gone(
    table: &InMemoryLocalPubSubDirectory,
    forwarder: &ControlMessageForwarder,
    publisher_session_id: SessionId,
    upstream_request_id: u64,
) -> bool {
    let publisher_session_gone = !forwarder
        .repository
        .lock()
        .await
        .has_session(publisher_session_id);
    if publisher_session_gone {
        tracing::debug!(
            publisher_session_id = %publisher_session_id,
            request_id = %upstream_request_id,
            "publisher session closed before its upstream subscription was registered"
        );
        table.end_upstream_subscription(
            publisher_session_id,
            upstream_request_id,
            PublishDoneReason::publisher_session_closed(),
        );
    }
    publisher_session_gone
}

pub(crate) struct UpstreamJoin {
    pub(crate) track_key: TrackKey,
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) pending: PendingUpstreamSubscribes,
}

#[derive(Clone)]
pub(crate) struct UpstreamJoinDeps {
    pub(crate) table: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) forwarder: ControlMessageForwarder,
    pub(crate) cache_store: Arc<TrackCacheStore>,
}

/// Adds every publisher that answers a pending upstream SUBSCRIBE to the
/// already answered track, so a publisher that never answers (e.g. a session
/// that died without closing) delays nobody.
pub(crate) struct UpstreamJoinTask {
    _join_handle: JoinHandle<()>,
}

impl UpstreamJoinTask {
    pub(crate) fn run(join: UpstreamJoin, deps: UpstreamJoinDeps) -> Self {
        let join_handle = tokio::spawn(Self::join(join, deps).instrument(Span::current()));
        Self {
            _join_handle: join_handle,
        }
    }

    async fn join(mut join: UpstreamJoin, deps: UpstreamJoinDeps) {
        while let Some(answered) = next_answered(&mut join.pending).await {
            match answered.subscribed {
                Ok(subscription) => {
                    join_track(
                        &deps,
                        &join.track_key,
                        answered.publisher_session_id,
                        join.subscriber_session_id,
                        subscribe_initiated(&subscription, answered.publisher_peer),
                    )
                    .await;
                }
                Err(error) => tracing::warn!(
                    %error,
                    publisher_session_id = %answered.publisher_session_id,
                    track_namespace = %join.track_key.track_namespace, track_name = %join.track_key.track_name,
                    "upstream SUBSCRIBE of an additional publisher failed"
                ),
            }
        }
    }
}

/// The ingest starts before the producer is added, since the producer owns
/// it; a track that stopped taking the publisher meanwhile drops the ingest
/// with the producer, and the publisher is unsubscribed either way.
async fn join_track(
    deps: &UpstreamJoinDeps,
    track_key: &TrackKey,
    publisher_session_id: SessionId,
    subscriber_session_id: SessionId,
    state: ProducerState,
) {
    let subscription = state.subscription.clone();
    let upstream_request_id = subscription.request_id();
    let joined = deps
        .table
        .track_takes_publisher(track_key, publisher_session_id, state.peer)
        && match start_ingest(
            &deps.forwarder,
            &deps.cache_store,
            IngestRequest {
                subscriber_session_id,
                publisher_session_id,
                track_key: track_key.clone(),
                subscription: subscription.clone(),
            },
        )
        .await
        {
            Some(ingest) => deps.table.add_upstream_subscription_to_track(
                track_key,
                publisher_session_id,
                Producer::new(state, ingest),
            ),
            None => return,
        };
    if !joined {
        tracing::debug!(
            publisher_session_id = %publisher_session_id,
            track_namespace = %track_key.track_namespace,
            track_name = %track_key.track_name,
            "unsubscribing a publisher the track does not take"
        );
        if let Err(error) = deps
            .forwarder
            .unsubscribe(publisher_session_id, &subscription)
            .await
        {
            tracing::warn!(
                ?error,
                "failed to unsubscribe the untaken upstream subscription"
            );
        }
        return;
    }
    if end_if_publisher_gone(
        &deps.table,
        &deps.forwarder,
        publisher_session_id,
        upstream_request_id,
    )
    .await
    {
        return;
    }
    tracing::info!(
        publisher_session_id = %publisher_session_id,
        track_namespace = %track_key.track_namespace,
        track_name = %track_key.track_name,
        "additional upstream publisher joined the track"
    );
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::directory_fixtures::delivery_control;
    use moqt::fetch::JoinPoint;
    use std::time::Duration;

    use super::*;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, local_publisher_resolver,
                table_with_upstream,
            },
            mock_session::{
                RecordedControlMessages, recorded_session_answering_subscribe,
                session_repository_with_session,
            },
        },
    };

    const LATE_PUBLISHER_SESSION: SessionId = 3;
    const LATE_UPSTREAM_REQUEST_ID: u64 = 1;

    fn deps(
        table: Arc<InMemoryLocalPubSubDirectory>,
        forwarder: ControlMessageForwarder,
    ) -> UpstreamJoinDeps {
        UpstreamJoinDeps {
            table,
            forwarder,
            cache_store: Arc::new(TrackCacheStore::new()),
        }
    }

    async fn join_after_answer(
        table: Arc<InMemoryLocalPubSubDirectory>,
        publisher_session_id: SessionId,
    ) -> RecordedControlMessages {
        let (session, recorded) =
            recorded_session_answering_subscribe(|| moqt::object::ContentExists::False);
        let repository = session_repository_with_session(
            publisher_session_id,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let forwarder = ControlMessageForwarder { repository };
        let pending = send_upstream_subscribes(
            &Arc::new(local_publisher_resolver()),
            &forwarder,
            &TrackKey::new("ns", "track"),
            vec![UpstreamPublisher::Session(publisher_session_id)],
        );
        let task = UpstreamJoinTask::run(
            UpstreamJoin {
                track_key: TrackKey::new("ns", "track"),
                subscriber_session_id: 2,
                pending,
            },
            deps(table, forwarder),
        );
        tokio::time::timeout(Duration::from_secs(1), task._join_handle)
            .await
            .expect("the join should end once every publisher answered")
            .unwrap();
        recorded
    }

    #[tokio::test]
    async fn a_publisher_answering_after_the_track_ended_is_unsubscribed() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());

        // Act
        let recorded = join_after_answer(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(recorded.data_receivers().is_empty());
        assert!(table.upstream_tracks.is_empty());
    }

    #[tokio::test]
    async fn a_publisher_already_feeding_the_track_keeps_its_ingest() {
        // Arrange
        let (table, _) = table_with_upstream(ProducerOrigin::Subscribe);
        let table = Arc::new(table);

        // Act
        let recorded = join_after_answer(table.clone(), PUBLISHER_SESSION).await;

        // Assert: the second answer is unsubscribed without an ingest; the first stays
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(recorded.data_receivers().is_empty());
        assert!(table.has_upstream_subscription(
            &TrackKey::new("ns", "track"),
            PUBLISHER_SESSION,
            UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_answering_while_the_track_lives_joins_it() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key,
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();
        let table = Arc::new(table);

        // Act
        let recorded = join_after_answer(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert!(recorded.unsubscribed_request_ids().is_empty());
        recorded.wait_ingest_started(LATE_UPSTREAM_REQUEST_ID).await;
        assert!(recorded.ingests_stopped().is_empty());
        assert!(table.has_upstream_subscription(
            &TrackKey::new("ns", "track"),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_answering_after_every_subscriber_left_is_unsubscribed() {
        // Arrange
        let (table, _) = table_with_upstream(ProducerOrigin::Publish);
        let table = Arc::new(table);

        // Act
        let recorded = join_after_answer(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(recorded.data_receivers().is_empty());
        assert!(!table.has_upstream_subscription(
            &TrackKey::new("ns", "track"),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_whose_session_ended_while_answering_is_not_kept_on_the_track() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();
        let table = Arc::new(table);
        let (session, recorded) =
            recorded_session_answering_subscribe(|| moqt::object::ContentExists::False);
        let repository = session_repository_with_session(
            LATE_PUBLISHER_SESSION,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let forwarder = ControlMessageForwarder { repository };
        let subscription = forwarder
            .subscribe(
                LATE_PUBLISHER_SESSION,
                "ns".to_string(),
                "track".to_string(),
            )
            .await
            .unwrap();
        forwarder
            .repository
            .lock()
            .await
            .remove(LATE_PUBLISHER_SESSION);
        let mut pending = JoinSet::new();
        pending.spawn(async move {
            Some(AnsweredUpstreamSubscribe {
                publisher_session_id: LATE_PUBLISHER_SESSION,
                publisher_peer: SessionPeer::Client,
                subscribed: Ok(subscription),
            })
        });

        // Act
        let task = UpstreamJoinTask::run(
            UpstreamJoin {
                track_key: track_key.clone(),
                subscriber_session_id: 2,
                pending,
            },
            deps(table.clone(), forwarder),
        );
        tokio::time::timeout(Duration::from_secs(1), task._join_handle)
            .await
            .unwrap()
            .unwrap();

        // Assert: a session that already left gets no ingest and no producer
        assert!(!table.has_upstream_subscription(
            &track_key,
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
        assert!(recorded.data_receivers().is_empty());
    }
}

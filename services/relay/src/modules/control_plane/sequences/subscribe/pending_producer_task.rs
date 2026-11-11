use std::sync::Arc;

use moqt::RequestErrorCode;
use tokio::task::JoinHandle;
use tracing::{Instrument, Span};

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{
            IngestRequest,
            malformed_track::{MalformedTrackCleanup, refused_as_malformed},
            session_peer, start_ingest,
            subscribe::UpstreamSubscriptionError,
        },
        upstream_publisher_resolver::{UpstreamPublisher, UpstreamPublisherResolver},
    },
    data_plane::cache::store::TrackCacheStore,
    directory::{
        InMemoryLocalPubSubDirectory,
        track::{CreationFailure, Producer, ProducerOrigin, ProducerState},
    },
    domain::{
        publish_done_reason::PublishDoneReason, session_id::SessionId, session_peer::SessionPeer,
        track_key::TrackKey,
    },
    session::subscription::UpstreamSubscription,
};

#[derive(Clone)]
pub(crate) struct PendingProducerDeps {
    pub(crate) table: Arc<InMemoryLocalPubSubDirectory>,
    pub(crate) forwarder: ControlMessageForwarder,
    pub(crate) cache_store: Arc<TrackCacheStore>,
    pub(crate) resolver: Arc<UpstreamPublisherResolver>,
}

/// Sends the track's upstream SUBSCRIBE to every publisher (draft-14 §8.4),
/// each in its own `PendingProducerTask`, so a relay that cannot be reached
/// or a session that never answers holds back no other. `new_group_request`
/// is the NEW_GROUP_REQUEST of the SUBSCRIBE that creates the track, which
/// the upstream SUBSCRIBEs carry too (draft-18 §10.2.13).
pub(crate) fn subscribe_publishers(
    deps: &PendingProducerDeps,
    track_key: &TrackKey,
    subscriber_session_id: SessionId,
    publishers: Vec<UpstreamPublisher>,
    new_group_request: Option<u64>,
) {
    deps.table
        .add_pending_producers(track_key, publishers.len());
    for publisher in publishers {
        let _task = PendingProducerTask::run(
            deps.clone(),
            track_key.clone(),
            subscriber_session_id,
            publisher,
            new_group_request,
        );
    }
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
        forward: true,
        on_demand: false,
    }
}

/// One upstream SUBSCRIBE in flight, counted among the track's pending
/// producers. SUBSCRIBE_OK starts the ingest and adds the producer; the first
/// producer completes the track's creation. A publisher the track no longer
/// takes is unsubscribed. A refusal as malformed latches the track
/// (draft-18 §2.4.2): its creation fails with MALFORMED_TRACK, every
/// producer is released, and a publisher answering later finds no track to
/// feed and is unsubscribed.
pub(crate) struct PendingProducerTask {
    _join_handle: JoinHandle<()>,
}

impl PendingProducerTask {
    pub(crate) fn run(
        deps: PendingProducerDeps,
        track_key: TrackKey,
        subscriber_session_id: SessionId,
        publisher: UpstreamPublisher,
        new_group_request: Option<u64>,
    ) -> Self {
        let join_handle = tokio::spawn(
            async move {
                let failure = Self::subscribe_once(
                    &deps,
                    &track_key,
                    subscriber_session_id,
                    &publisher,
                    new_group_request,
                )
                .await;
                let malformed = failure
                    .as_ref()
                    .is_some_and(|failure| failure.code == RequestErrorCode::MalformedTrack);
                deps.table.pending_producer_ended(&track_key, failure);
                if malformed {
                    MalformedTrackCleanup
                        .handle(&track_key, &deps.table, &deps.forwarder)
                        .await;
                }
            }
            .instrument(Span::current()),
        );
        Self {
            _join_handle: join_handle,
        }
    }

    /// `Some` when no producer came of this SUBSCRIBE because of a failure
    /// a subscriber waiting for the track should hear about.
    async fn subscribe_once(
        deps: &PendingProducerDeps,
        track_key: &TrackKey,
        subscriber_session_id: SessionId,
        publisher: &UpstreamPublisher,
        new_group_request: Option<u64>,
    ) -> Option<CreationFailure> {
        let Some(publisher_session_id) = deps.resolver.session_of(publisher).await else {
            return Some(CreationFailure {
                code: RequestErrorCode::InternalError,
                reason: "no upstream publisher could be reached".to_string(),
            });
        };
        if !deps
            .table
            .begin_upstream_subscribe(track_key, publisher_session_id)
        {
            tracing::debug!(
                publisher_session_id = %publisher_session_id,
                track_namespace = %track_key.track_namespace,
                track_name = %track_key.track_name,
                "upstream SUBSCRIBE skipped: the track is gone or the publisher is already subscribed"
            );
            return None;
        }
        let failure = Self::answer(
            deps,
            track_key,
            subscriber_session_id,
            publisher_session_id,
            new_group_request,
        )
        .await;
        deps.table
            .end_upstream_subscribe(track_key, publisher_session_id);
        failure
    }

    async fn answer(
        deps: &PendingProducerDeps,
        track_key: &TrackKey,
        subscriber_session_id: SessionId,
        publisher_session_id: SessionId,
        new_group_request: Option<u64>,
    ) -> Option<CreationFailure> {
        let publisher_peer = session_peer(publisher_session_id, &deps.forwarder).await;
        let subscription = match deps
            .forwarder
            .subscribe(
                publisher_session_id,
                track_key.track_namespace.clone(),
                track_key.track_name.clone(),
                new_group_request,
            )
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                let malformed = refused_as_malformed(&error);
                tracing::warn!(
                    %error,
                    malformed,
                    publisher_session_id = %publisher_session_id,
                    track_namespace = %track_key.track_namespace,
                    track_name = %track_key.track_name,
                    "upstream SUBSCRIBE failed"
                );
                if malformed {
                    deps.cache_store.get_or_create(track_key).mark_malformed();
                }
                return Some(UpstreamSubscriptionError::SubscribeFailed(error).failure());
            }
        };
        tracing::info!(
            publisher_session_id = %publisher_session_id,
            track_namespace = %track_key.track_namespace,
            track_name = %track_key.track_name,
            track_alias = subscription.track_alias(),
            expires = subscription.expires().unwrap_or(0),
            "upstream SUBSCRIBE_OK received"
        );

        // The publisher sent PUBLISH for the track while the SUBSCRIBE was
        // pending: unsubscribing that PUBLISH would fail the publisher's writes.
        if deps
            .table
            .published_upstream_subscription(track_key, publisher_session_id)
            .is_some()
        {
            unsubscribe(
                &deps.forwarder,
                publisher_session_id,
                &subscription,
                "the SUBSCRIBE a PUBLISH overtook",
            )
            .await;
            return None;
        }
        let upstream_request_id = subscription.request_id();
        let joined =
            deps.table
                .track_takes_publisher(track_key, publisher_session_id, publisher_peer)
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
                        Producer::new(subscribe_initiated(&subscription, publisher_peer), ingest),
                    ),
                    None => return Some(UpstreamSubscriptionError::PublisherNotFound.failure()),
                };
        if !joined {
            unsubscribe(
                &deps.forwarder,
                publisher_session_id,
                &subscription,
                "a publisher the track does not take",
            )
            .await;
            return None;
        }
        if end_if_publisher_gone(
            &deps.table,
            &deps.forwarder,
            publisher_session_id,
            upstream_request_id,
        )
        .await
        {
            return Some(UpstreamSubscriptionError::PublisherNotFound.failure());
        }
        tracing::info!(
            publisher_session_id = %publisher_session_id,
            track_namespace = %track_key.track_namespace,
            track_name = %track_key.track_name,
            "upstream publisher joined the track"
        );
        None
    }
}

async fn unsubscribe(
    forwarder: &ControlMessageForwarder,
    publisher_session_id: SessionId,
    subscription: &UpstreamSubscription,
    what: &'static str,
) {
    if let Err(error) = forwarder
        .unsubscribe(publisher_session_id, subscription)
        .await
    {
        tracing::warn!(
            ?error,
            publisher_session_id = %publisher_session_id,
            request_id = subscription.request_id(),
            what,
            "failed to unsubscribe"
        );
    }
}

/// Ends the registered upstream subscription, and with it its ingest, when
/// the publisher session has left the repository; returns true in that case.
/// Call it after the producer was added: a session leaves the repository
/// before its cleanup reads the directory, so either this check or the
/// cleanup sees the producer.
async fn end_if_publisher_gone(
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

#[cfg(test)]
mod tests {
    use crate::modules::test_support::directory_fixtures::downstream_subscription;
    use std::{sync::OnceLock, time::Duration};

    use moqt::object::ContentExists;

    use super::*;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        directory::{TrackLookup, track::CreationState},
        session::{Session, session_repository::SessionRepository},
        test_support::{
            directory_fixtures::{
                PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, delivery_control, local_publisher_resolver,
                table_with_upstream, track_key,
            },
            mock_session::{
                RecordedControlMessages, mock_session_answering_subscribe,
                recorded_session_answering_subscribe, recorded_session_answering_subscribe_after,
                session_repository_with_session, wait_until,
            },
        },
    };

    const SUBSCRIBER_SESSION: SessionId = 2;
    const LATE_PUBLISHER_SESSION: SessionId = 3;
    /// The mock answers every SUBSCRIBE with request id 1.
    const LATE_UPSTREAM_REQUEST_ID: u64 = 1;

    fn deps(
        table: Arc<InMemoryLocalPubSubDirectory>,
        repository: Arc<tokio::sync::Mutex<SessionRepository>>,
    ) -> PendingProducerDeps {
        PendingProducerDeps {
            table,
            forwarder: ControlMessageForwarder { repository },
            cache_store: Arc::new(TrackCacheStore::new()),
            resolver: Arc::new(local_publisher_resolver()),
        }
    }

    /// Runs one `PendingProducerTask` against `session`, which answers
    /// SUBSCRIBE_OK, until the task ends.
    async fn answer_from(
        table: Arc<InMemoryLocalPubSubDirectory>,
        publisher_session_id: SessionId,
        session: Box<dyn Session>,
    ) {
        let repository = session_repository_with_session(
            publisher_session_id,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        table.add_pending_producers(&track_key(), 1);
        let task = PendingProducerTask::run(
            deps(table, repository),
            track_key(),
            SUBSCRIBER_SESSION,
            UpstreamPublisher::Session(publisher_session_id),
            None,
        );
        tokio::time::timeout(Duration::from_secs(1), task._join_handle)
            .await
            .expect("the SUBSCRIBE should be answered")
            .unwrap();
    }

    async fn answer_recorded(
        table: Arc<InMemoryLocalPubSubDirectory>,
        publisher_session_id: SessionId,
    ) -> RecordedControlMessages {
        let (session, recorded) = recorded_session_answering_subscribe(|| ContentExists::False);
        answer_from(table, publisher_session_id, session).await;
        recorded
    }

    #[tokio::test]
    async fn the_first_answer_creates_the_track() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        let TrackLookup::Started(creation, _reservation) =
            table.track_or_begin_creation(&track_key(), SessionPeer::Client)
        else {
            panic!("the first SUBSCRIBE starts the creation");
        };

        // Act
        let recorded = answer_recorded(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert_eq!(*creation.borrow(), CreationState::Created);
        assert!(table.has_upstream_subscription(
            &track_key(),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
        recorded.wait_ingest_started(LATE_UPSTREAM_REQUEST_ID).await;
    }

    #[tokio::test]
    async fn a_publisher_answering_after_the_track_ended_is_unsubscribed() {
        // Arrange: the track ends as the publisher sends SUBSCRIBE_OK
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        let TrackLookup::Started(_creation, _reservation) =
            table.track_or_begin_creation(&track_key(), SessionPeer::Client)
        else {
            panic!("the first SUBSCRIBE starts the creation");
        };
        let ending_table = table.clone();
        let (session, recorded) = recorded_session_answering_subscribe(move || {
            ending_table.remove_upstream_track(&track_key());
            ContentExists::False
        });

        // Act
        answer_from(table.clone(), LATE_PUBLISHER_SESSION, session).await;

        // Assert
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(recorded.data_receivers().is_empty());
        assert!(table.upstream_tracks.is_empty());
    }

    #[tokio::test]
    async fn a_publisher_already_feeding_the_track_is_not_subscribed_again() {
        // Arrange
        let (table, _) = table_with_upstream(ProducerOrigin::Subscribe);
        let table = Arc::new(table);

        // Act
        let recorded = answer_recorded(table.clone(), PUBLISHER_SESSION).await;

        // Assert
        assert!(recorded.subscribed_track_names().is_empty());
        assert!(table.has_upstream_subscription(
            &track_key(),
            PUBLISHER_SESSION,
            UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_with_a_subscribe_in_flight_is_not_subscribed_again() {
        // Arrange: the first SUBSCRIBE waits for its answer
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        let TrackLookup::Started(creation, _reservation) =
            table.track_or_begin_creation(&track_key(), SessionPeer::Client)
        else {
            panic!("the first SUBSCRIBE starts the creation");
        };
        let gate = Arc::new(tokio::sync::Notify::new());
        let (session, recorded) = recorded_session_answering_subscribe_after(gate.clone());
        let repository = session_repository_with_session(
            LATE_PUBLISHER_SESSION,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let deps = deps(table.clone(), repository);
        table.add_pending_producers(&track_key(), 2);
        let first = PendingProducerTask::run(
            deps.clone(),
            track_key(),
            SUBSCRIBER_SESSION,
            UpstreamPublisher::Session(LATE_PUBLISHER_SESSION),
            None,
        );
        wait_until(|| recorded.subscribed_track_names().len() == 1)
            .await
            .expect("the first SUBSCRIBE should be sent");

        // Act
        let second = PendingProducerTask::run(
            deps,
            track_key(),
            SUBSCRIBER_SESSION,
            UpstreamPublisher::Session(LATE_PUBLISHER_SESSION),
            None,
        );
        tokio::time::timeout(Duration::from_secs(1), second._join_handle)
            .await
            .expect("the second task should end without waiting for the first")
            .unwrap();
        gate.notify_one();
        first._join_handle.await.unwrap();

        // Assert
        assert_eq!(recorded.subscribed_track_names().len(), 1);
        assert!(recorded.unsubscribed_request_ids().is_empty());
        assert_eq!(*creation.borrow(), CreationState::Created);
        assert!(table.has_upstream_subscription(
            &track_key(),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_answering_while_the_track_lives_joins_it() {
        // Arrange
        let (table, _) = table_with_upstream(ProducerOrigin::Subscribe);
        table
            .register_downstream_subscription(
                SUBSCRIBER_SESSION,
                100,
                SessionPeer::Client,
                downstream_subscription(track_key(), None),
                delivery_control(true),
            )
            .unwrap();
        let table = Arc::new(table);

        // Act
        let recorded = answer_recorded(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert!(recorded.unsubscribed_request_ids().is_empty());
        recorded.wait_ingest_started(LATE_UPSTREAM_REQUEST_ID).await;
        assert!(recorded.ingests_stopped().is_empty());
        assert!(table.has_upstream_subscription(
            &track_key(),
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
        let recorded = answer_recorded(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(recorded.data_receivers().is_empty());
        assert!(!table.has_upstream_subscription(
            &track_key(),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_whose_session_ended_while_answering_is_not_kept_on_the_track() {
        // Arrange: the session leaves the repository as it sends SUBSCRIBE_OK
        let (table, _) = table_with_upstream(ProducerOrigin::Subscribe);
        table
            .register_downstream_subscription(
                SUBSCRIBER_SESSION,
                100,
                SessionPeer::Client,
                downstream_subscription(track_key(), None),
                delivery_control(true),
            )
            .unwrap();
        let table = Arc::new(table);
        let cell = Arc::new(OnceLock::<Arc<tokio::sync::Mutex<SessionRepository>>>::new());
        let closing_cell = cell.clone();
        let closing_session = mock_session_answering_subscribe(move || {
            closing_cell
                .get()
                .unwrap()
                .try_lock()
                .unwrap()
                .remove(LATE_PUBLISHER_SESSION);
            ContentExists::False
        });
        let repository = session_repository_with_session(
            LATE_PUBLISHER_SESSION,
            closing_session,
            VerifiedToken::full_access(),
        )
        .await;
        let _ = cell.set(repository.clone());
        table.add_pending_producers(&track_key(), 1);

        // Act
        let task = PendingProducerTask::run(
            deps(table.clone(), repository),
            track_key(),
            SUBSCRIBER_SESSION,
            UpstreamPublisher::Session(LATE_PUBLISHER_SESSION),
            None,
        );
        tokio::time::timeout(Duration::from_secs(1), task._join_handle)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert!(!table.has_upstream_subscription(
            &track_key(),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
    }
}

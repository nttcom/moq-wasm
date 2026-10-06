use std::sync::Arc;

use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tracing::{Instrument, Span};

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{session_peer, start_ingress, stop_ingress},
        upstream_publisher_resolver::{UpstreamPublisher, UpstreamPublisherResolver},
    },
    data_plane::ingress::ingress_coordinator::{IngressCommand, IngressStartRequest},
    domain::{
        pub_sub_directory::{
            InMemoryLocalPubSubDirectory,
            entry::{ActiveUpstreamSubscription, PublishDoneReason, UpstreamSubscriptionOrigin},
        },
        session_id::SessionId,
        session_peer::SessionPeer,
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
) -> ActiveUpstreamSubscription {
    ActiveUpstreamSubscription {
        upstream_request_id: subscription.request_id(),
        expires: subscription.expires(),
        content_exists: subscription.content_exists(),
        origin: UpstreamSubscriptionOrigin::Subscribe,
        publisher_peer,
    }
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
    pub(crate) ingress_sender: mpsc::Sender<IngressCommand>,
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
                        subscription,
                    )
                    .await;
                }
                Err(error) => tracing::warn!(
                    %error,
                    pub_session_id = %answered.publisher_session_id,
                    track_key = %join.track_key,
                    "upstream SUBSCRIBE of an additional publisher failed"
                ),
            }
        }
    }
}

/// The subscription is added before ingress starts, so a publisher already
/// feeding the track keeps its ingest; a removal racing the start is caught by
/// the check after it.
async fn join_track(
    deps: &UpstreamJoinDeps,
    track_key: &TrackKey,
    publisher_session_id: SessionId,
    subscriber_session_id: SessionId,
    active_upstream: ActiveUpstreamSubscription,
    subscription: UpstreamSubscription,
) {
    let upstream_request_id = active_upstream.upstream_request_id;
    let joined = deps.table.add_upstream_subscription_to_track(
        track_key,
        publisher_session_id,
        active_upstream,
    );
    if !joined {
        tracing::info!(
            pub_session_id = %publisher_session_id,
            %track_key,
            "unsubscribing a publisher the track does not take"
        );
        if let Err(error) = deps
            .forwarder
            .unsubscribe(publisher_session_id, upstream_request_id)
            .await
        {
            tracing::warn!(
                ?error,
                "failed to unsubscribe the untaken upstream subscription"
            );
        }
        return;
    }
    if !start_ingress(
        &deps.ingress_sender,
        IngressStartRequest {
            subscriber_session_id,
            publisher_session_id,
            track_key: track_key.clone(),
            subscription,
            parent_span: Span::current(),
        },
    )
    .await
    {
        return;
    }
    if !deps
        .table
        .has_upstream_subscription(track_key, publisher_session_id, upstream_request_id)
    {
        stop_ingress(&deps.ingress_sender, publisher_session_id, track_key).await;
        return;
    }
    let publisher_session_gone = !deps
        .forwarder
        .repository
        .lock()
        .await
        .has_session(publisher_session_id);
    if publisher_session_gone {
        deps.table.end_upstream_subscription(
            publisher_session_id,
            upstream_request_id,
            PublishDoneReason::publisher_session_closed(),
        );
        stop_ingress(&deps.ingress_sender, publisher_session_id, track_key).await;
        return;
    }
    tracing::info!(
        pub_session_id = %publisher_session_id,
        %track_key,
        "additional upstream publisher joined the track"
    );
}

#[cfg(test)]
mod tests {
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

    async fn join_after_answer(
        table: Arc<InMemoryLocalPubSubDirectory>,
        publisher_session_id: SessionId,
    ) -> (RecordedControlMessages, mpsc::Receiver<IngressCommand>) {
        let (session, recorded) = recorded_session_answering_subscribe();
        let repository = session_repository_with_session(
            publisher_session_id,
            session,
            VerifiedToken::full_access(),
        )
        .await;
        let forwarder = ControlMessageForwarder { repository };
        let (ingress_sender, ingress_receiver) = mpsc::channel(8);
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
            UpstreamJoinDeps {
                table,
                forwarder,
                ingress_sender,
            },
        );
        tokio::time::timeout(Duration::from_secs(1), task._join_handle)
            .await
            .expect("the join should end once every publisher answered")
            .unwrap();
        (recorded, ingress_receiver)
    }

    #[tokio::test]
    async fn a_publisher_answering_after_the_track_ended_is_unsubscribed() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());

        // Act
        let (recorded, mut ingress_receiver) =
            join_after_answer(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(ingress_receiver.try_recv().is_err());
        assert!(table.upstream_tracks.is_empty());
    }

    #[tokio::test]
    async fn a_publisher_already_feeding_the_track_keeps_its_ingest() {
        // Arrange
        let (table, _) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let table = Arc::new(table);

        // Act
        let (recorded, mut ingress_receiver) =
            join_after_answer(table.clone(), PUBLISHER_SESSION).await;

        // Assert
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(ingress_receiver.try_recv().is_err());
        assert!(table.has_upstream_subscription(
            &TrackKey::new("ns", "track"),
            PUBLISHER_SESSION,
            UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_answering_while_the_track_lives_joins_it() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table
            .register_downstream_subscription(2, 100, SessionPeer::Client, track_key, None)
            .unwrap();
        let table = Arc::new(table);

        // Act
        let (recorded, mut ingress_receiver) =
            join_after_answer(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert!(recorded.unsubscribed_request_ids().is_empty());
        assert!(matches!(
            ingress_receiver.try_recv(),
            Ok(IngressCommand::Start(request)) if request.publisher_session_id == LATE_PUBLISHER_SESSION
        ));
        assert!(table.has_upstream_subscription(
            &TrackKey::new("ns", "track"),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_answering_after_every_subscriber_left_is_unsubscribed() {
        // Arrange
        let (table, _) = table_with_upstream(UpstreamSubscriptionOrigin::Publish);
        let table = Arc::new(table);

        // Act
        let (recorded, mut ingress_receiver) =
            join_after_answer(table.clone(), LATE_PUBLISHER_SESSION).await;

        // Assert
        assert_eq!(
            recorded.unsubscribed_request_ids(),
            vec![LATE_UPSTREAM_REQUEST_ID]
        );
        assert!(ingress_receiver.try_recv().is_err());
        assert!(!table.has_upstream_subscription(
            &TrackKey::new("ns", "track"),
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
    }

    #[tokio::test]
    async fn a_publisher_whose_session_ended_while_answering_is_not_kept_on_the_track() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table
            .register_downstream_subscription(2, 100, SessionPeer::Client, track_key.clone(), None)
            .unwrap();
        let table = Arc::new(table);
        let (session, _recorded) = recorded_session_answering_subscribe();
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
        let (ingress_sender, mut ingress_receiver) = mpsc::channel(8);

        // Act
        let task = UpstreamJoinTask::run(
            UpstreamJoin {
                track_key: track_key.clone(),
                subscriber_session_id: 2,
                pending,
            },
            UpstreamJoinDeps {
                table: table.clone(),
                forwarder,
                ingress_sender,
            },
        );
        tokio::time::timeout(Duration::from_secs(1), task._join_handle)
            .await
            .unwrap()
            .unwrap();

        // Assert
        assert!(!table.has_upstream_subscription(
            &track_key,
            LATE_PUBLISHER_SESSION,
            LATE_UPSTREAM_REQUEST_ID
        ));
        assert!(matches!(
            ingress_receiver.try_recv(),
            Ok(IngressCommand::Start(_))
        ));
        assert!(matches!(
            ingress_receiver.try_recv(),
            Ok(IngressCommand::StopTrack { publisher_session_id, .. })
                if publisher_session_id == LATE_PUBLISHER_SESSION
        ));
    }
}

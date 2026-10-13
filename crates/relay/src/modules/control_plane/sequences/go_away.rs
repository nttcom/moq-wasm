use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::Span;

use crate::modules::{
    control_plane::{
        control_message_forwarder::ControlMessageForwarder,
        sequences::{
            session_peer,
            subscribe::upstream_join_task::{
                UpstreamJoin, UpstreamJoinDeps, UpstreamJoinTask, send_upstream_subscribes,
            },
        },
        upstream_publisher_resolver::{UpstreamPublisher, UpstreamPublisherResolver},
    },
    data_plane::ingress::ingress_coordinator::IngressCommand,
    domain::{
        pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId,
        session_peer::SessionPeer, track_key::TrackKey,
    },
};

pub(crate) struct GoAway;

impl GoAway {
    /// draft-14 §3.5: an upstream relay that is going away will close its
    /// session. Before that happens, every track it feeds is offered to the
    /// other publishers the resolver knows (local client publishers and the
    /// relays still active in the route registry); a publisher that already
    /// feeds the track is left alone. A client sending GOAWAY only announces
    /// that it leaves; nothing is resolved for it.
    #[tracing::instrument(
        level = "info",
        name = "relay.sequence.go_away",
        skip_all,
        parent = session_span,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        session_span: &Span,
        table: &Arc<InMemoryLocalPubSubDirectory>,
        forwarder: &ControlMessageForwarder,
        ingress_sender: &mpsc::Sender<IngressCommand>,
        upstream_publisher_resolver: &Arc<UpstreamPublisherResolver>,
    ) {
        if session_peer(session_id, forwarder).await == SessionPeer::Client {
            tracing::info!("client announced that it is leaving");
            return;
        }
        let fed_tracks: Vec<TrackKey> = table
            .active_upstream_tracks()
            .into_iter()
            .filter(|(publisher_session_id, _)| *publisher_session_id == session_id)
            .map(|(_, track_key)| track_key)
            .collect();
        tracing::info!(
            tracks = fed_tracks.len(),
            "upstream relay is going away; subscribing the other publishers of its tracks"
        );
        let join_deps = UpstreamJoinDeps {
            table: table.clone(),
            forwarder: forwarder.clone(),
            ingress_sender: ingress_sender.clone(),
        };
        for track_key in fed_tracks {
            let feeding = table
                .get_upstream_track(&track_key)
                .map(|track| track.subscriptions.keys().copied().collect::<Vec<_>>())
                .unwrap_or_default();
            let publishers: Vec<UpstreamPublisher> = upstream_publisher_resolver
                .resolve(
                    table,
                    &track_key.track_namespace,
                    &track_key.track_name,
                    SessionPeer::Client,
                )
                .await
                .into_iter()
                .filter(|publisher| {
                    !matches!(publisher, UpstreamPublisher::Session(id) if feeding.contains(id))
                })
                .collect();
            if publishers.is_empty() {
                tracing::warn!(%track_key, "no other publisher for a track of the departing relay");
                continue;
            }
            tracing::info!(%track_key, publishers = publishers.len(), "subscribing other publishers");
            let pending = send_upstream_subscribes(
                upstream_publisher_resolver,
                &join_deps.forwarder,
                &track_key,
                publishers,
            );
            let _upstream_join = UpstreamJoinTask::run(
                UpstreamJoin {
                    subscriber_session_id: session_id,
                    track_key,
                    pending,
                },
                join_deps.clone(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use tokio::sync::mpsc;

    use super::GoAway;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        control_plane::control_message_forwarder::ControlMessageForwarder,
        domain::{
            pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::UpstreamSubscriptionOrigin},
            session_id::SessionId,
            session_peer::SessionPeer,
        },
        session::session_repository::{NewSession, SessionRepository},
        test_support::{
            directory_fixtures::{active_upstream, local_publisher_resolver, track_key},
            mock_session::{
                RecordedControlMessages, mock_new_session, recorded_session_answering_subscribe,
            },
        },
    };

    const DEPARTING_RELAY: SessionId = 9;
    const OTHER_PUBLISHER: SessionId = 3;

    /// `DEPARTING_RELAY` with the given peer feeding `ns/track`, and client
    /// `OTHER_PUBLISHER` publishing the namespace and answering SUBSCRIBE.
    async fn context(
        departing_peer: SessionPeer,
    ) -> (
        Arc<InMemoryLocalPubSubDirectory>,
        ControlMessageForwarder,
        RecordedControlMessages,
    ) {
        let mut repository = SessionRepository::new();
        let (event_sender, _event_receiver) = mpsc::unbounded_channel();
        let (mut departing, _) = mock_new_session(DEPARTING_RELAY, VerifiedToken::full_access());
        departing.peer = departing_peer;
        repository.add(departing, event_sender.clone()).await;
        let (other, recorded) = recorded_session_answering_subscribe();
        repository
            .add(
                NewSession {
                    session_id: OTHER_PUBLISHER,
                    session: other,
                    session_span: tracing::Span::none(),
                    peer: SessionPeer::Client,
                    verified_token: VerifiedToken::full_access(),
                },
                event_sender,
            )
            .await;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_upstream_subscription(
            track_key(),
            DEPARTING_RELAY,
            active_upstream(UpstreamSubscriptionOrigin::Subscribe),
        );
        table.register_publish_namespace(OTHER_PUBLISHER, "ns".to_string(), SessionPeer::Client);
        let forwarder = ControlMessageForwarder {
            repository: Arc::new(tokio::sync::Mutex::new(repository)),
        };
        (table, forwarder, recorded)
    }

    async fn wait_for_unsubscribe(recorded: &RecordedControlMessages) -> bool {
        tokio::time::timeout(Duration::from_secs(1), async {
            while recorded.unsubscribed_request_ids().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn go_away_from_an_upstream_relay_subscribes_the_other_publishers_of_its_tracks() {
        // Arrange
        let (table, forwarder, recorded) = context(SessionPeer::Relay).await;
        let (ingress_sender, _ingress_receiver) = mpsc::channel(8);
        let resolver = Arc::new(local_publisher_resolver());

        // Act
        GoAway
            .handle(
                DEPARTING_RELAY,
                &tracing::Span::none(),
                &table,
                &forwarder,
                &ingress_sender,
                &resolver,
            )
            .await;

        // Assert: the other publisher was subscribed; nobody watches the track,
        // so the join released that subscription again
        assert!(wait_for_unsubscribe(&recorded).await);
    }

    #[tokio::test]
    async fn go_away_from_a_client_resolves_nothing() {
        // Arrange
        let (table, forwarder, recorded) = context(SessionPeer::Client).await;
        let (ingress_sender, _ingress_receiver) = mpsc::channel(8);
        let resolver = Arc::new(local_publisher_resolver());

        // Act
        GoAway
            .handle(
                DEPARTING_RELAY,
                &tracing::Span::none(),
                &table,
                &forwarder,
                &ingress_sender,
                &resolver,
            )
            .await;

        // Assert
        assert!(!wait_for_unsubscribe(&recorded).await);
    }
}

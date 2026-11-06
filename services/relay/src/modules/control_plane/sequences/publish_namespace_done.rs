use std::collections::HashSet;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::RelayRouteRegistry,
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder, sequences::CascadingRelayContext,
    },
    directory::{InMemoryLocalPubSubDirectory, PeerRemoval},
    domain::session_id::SessionId,
};

pub(crate) struct PublishNamespaceDone;

impl PublishNamespaceDone {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        cascading_relay_context: CascadingRelayContext<'_>,
        handler: &moqt::namespace::PublishNamespaceDoneHandler,
    ) {
        let track_namespace = handler.track_namespace();
        tracing::info!(
            session_id = %session_id,
            track_namespace = %track_namespace,
            "PUBLISH_NAMESPACE_DONE received"
        );

        let removal = table.unregister_publish_namespace(session_id, track_namespace);
        let is_origin_client = super::is_origin_client(session_id, forwarder).await;
        // A relay session may withdraw a namespace this relay never registered
        // for it, so only a client's DONE requires its own registration.
        if is_origin_client && removal == PeerRemoval::NotRegistered {
            tracing::warn!(
                session_id = %session_id,
                track_namespace = %track_namespace,
                "PUBLISH_NAMESPACE_DONE for a namespace the session does not publish; ignored"
            );
            return;
        }

        Self::notify_local_subscribers(session_id, track_namespace, table, forwarder).await;

        if is_origin_client && removal == PeerRemoval::NoClientsRemain {
            Self::withdraw_namespace_publication(
                track_namespace,
                forwarder,
                cascading_relay_context.route_registry,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }
    }

    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.publish_namespace_done.notify_local_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    pub(crate) async fn notify_local_subscribers(
        publisher_session_id: SessionId,
        track_namespace: &str,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
    ) {
        // Relay sessions count as publishers too (draft-14 §9.28).
        if table.has_namespace_publishers(track_namespace) {
            tracing::debug!(
                track_namespace = %track_namespace,
                "namespace still published by another session; PUBLISH_NAMESPACE_DONE not forwarded"
            );
            return;
        }
        for subscriber_session_id in table.get_namespace_subscribers(track_namespace) {
            if subscriber_session_id == publisher_session_id {
                continue;
            }
            forwarder
                .publish_namespace_done(subscriber_session_id, track_namespace.to_string())
                .await;
        }
    }

    /// Unregisters the Redis publisher route and tells remote subscriber relays
    /// that the namespace has been withdrawn, so they can drop their local copy.
    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.publish_namespace_done.withdraw_namespace_publication",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    pub(crate) async fn withdraw_namespace_publication(
        track_namespace: &str,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        if let Err(err) = route_registry
            .unregister_namespace_publisher(track_namespace)
            .await
        {
            tracing::warn!(
                error = ?err,
                track_namespace = %track_namespace,
                "failed to unregister namespace publisher route"
            );
        }

        let relays = match route_registry
            .find_namespace_subscribers(track_namespace)
            .await
        {
            Ok(relays) => relays,
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    track_namespace = %track_namespace,
                    "failed to find remote namespace subscribers for PUBLISH_NAMESPACE_DONE"
                );
                return;
            }
        };

        let mut notified_relay_ids = HashSet::new();
        for relay in relays {
            if !notified_relay_ids.insert(relay.relay_id.clone()) {
                continue;
            }

            let Some(session_id) =
                super::connect_relay(inter_relay_connection_manager, &relay).await
            else {
                continue;
            };

            forwarder
                .publish_namespace_done(session_id, track_namespace.to_string())
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use moqt::namespace::PublishNamespaceDoneHandler;

    use super::*;
    use crate::modules::{
        auth::verified_token::VerifiedToken,
        cascading::route_registry::NoopRelayRouteRegistry,
        domain::session_peer::SessionPeer,
        session::session_repository::NewSession,
        test_support::mock_session::{
            RecordedControlMessages, detached_session_repository,
            mock_session_with_transport_stats, recorded_session_answering_subscribe,
            session_repository_with_sessions,
        },
    };

    const NAMESPACE: &str = "room/alice";
    const LEAVING_PUBLISHER: SessionId = 1;
    const REMAINING_PUBLISHER: SessionId = 2;
    const NAMESPACE_SUBSCRIBER: SessionId = 3;

    async fn withdraw(
        table: &InMemoryLocalPubSubDirectory,
        publisher_peer: SessionPeer,
    ) -> RecordedControlMessages {
        let (subscriber_session, recorded) =
            recorded_session_answering_subscribe(|| moqt::object::ContentExists::False);
        let repository = session_repository_with_sessions(
            vec![(NAMESPACE_SUBSCRIBER, subscriber_session)],
            VerifiedToken::full_access(),
        )
        .await;
        repository.lock().await.add(NewSession {
            session_id: LEAVING_PUBLISHER,
            session: mock_session_with_transport_stats(Default::default()),
            session_span: tracing::Span::none(),
            peer: publisher_peer,
            verified_token: VerifiedToken::full_access(),
        });
        let inter_relay_connection_manager = InterRelayConnectionManager::new(
            Arc::new(tokio::sync::Mutex::new(detached_session_repository())),
            "unused-relay-token".to_string(),
        );
        let forwarder = ControlMessageForwarder { repository };
        forwarder
            .publish_namespace(NAMESPACE_SUBSCRIBER, NAMESPACE.to_string())
            .await;
        PublishNamespaceDone
            .handle(
                LEAVING_PUBLISHER,
                table,
                &forwarder,
                CascadingRelayContext {
                    route_registry: &NoopRelayRouteRegistry,
                    inter_relay_connection_manager: &inter_relay_connection_manager,
                },
                &PublishNamespaceDoneHandler {
                    track_namespace: NAMESPACE.to_string(),
                    track_namespace_tuple: NAMESPACE.split('/').map(str::to_string).collect(),
                },
            )
            .await;
        recorded
    }

    fn table_with_namespace_publishers(
        publishers: &[(SessionId, SessionPeer)],
    ) -> InMemoryLocalPubSubDirectory {
        let table = InMemoryLocalPubSubDirectory::new();
        for &(publisher, peer) in publishers {
            table.register_publish_namespace(publisher, NAMESPACE.to_string(), peer);
        }
        table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER,
            "room".to_string(),
            SessionPeer::Client,
        );
        table
    }

    #[tokio::test]
    async fn subscribers_are_not_told_done_while_another_session_publishes_the_namespace() {
        // Arrange
        let table = table_with_namespace_publishers(&[
            (LEAVING_PUBLISHER, SessionPeer::Client),
            (REMAINING_PUBLISHER, SessionPeer::Client),
        ]);

        // Act
        let recorded = withdraw(&table, SessionPeer::Client).await;

        // Assert
        assert!(recorded.publish_namespace_dones().is_empty());
    }

    #[tokio::test]
    async fn subscribers_are_not_told_done_while_another_relay_publishes_the_namespace() {
        // Arrange
        let table = table_with_namespace_publishers(&[
            (LEAVING_PUBLISHER, SessionPeer::Client),
            (REMAINING_PUBLISHER, SessionPeer::Relay),
        ]);

        // Act
        let recorded = withdraw(&table, SessionPeer::Client).await;

        // Assert
        assert!(recorded.publish_namespace_dones().is_empty());
    }

    #[tokio::test]
    async fn subscribers_are_told_done_when_the_last_publisher_withdraws() {
        // Arrange
        let table = table_with_namespace_publishers(&[(LEAVING_PUBLISHER, SessionPeer::Client)]);

        // Act
        let recorded = withdraw(&table, SessionPeer::Client).await;

        // Assert
        assert_eq!(
            recorded.publish_namespace_dones(),
            vec![NAMESPACE.to_string()]
        );
    }

    #[tokio::test]
    async fn subscribers_are_not_told_done_by_a_client_that_never_published_the_namespace() {
        // Arrange: the namespace is known only from another relay
        let table = table_with_namespace_publishers(&[]);

        // Act
        let recorded = withdraw(&table, SessionPeer::Client).await;

        // Assert
        assert!(recorded.publish_namespace_dones().is_empty());
    }

    #[tokio::test]
    async fn subscribers_are_told_done_by_a_relay_without_a_local_registration() {
        // Arrange
        let table = table_with_namespace_publishers(&[]);

        // Act
        let recorded = withdraw(&table, SessionPeer::Relay).await;

        // Assert
        assert_eq!(
            recorded.publish_namespace_dones(),
            vec![NAMESPACE.to_string()]
        );
    }
}

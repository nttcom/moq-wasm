use std::collections::HashSet;

use moqt::namespace::NamespaceInterest;

use crate::modules::{
    cascading::inter_relay_connection_manager::InterRelayConnectionManager,
    control_plane::{
        control_message_forwarder::ControlMessageForwarder, sequences::CascadingRelayContext,
    },
    directory::{InMemoryLocalPubSubDirectory, PeerRemoval},
    domain::{session_id::SessionId, session_peer::SessionPeer},
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
        let track_namespace = handler.track_namespace.as_str();
        let request_id = handler.request_id;
        tracing::info!(
            session_id = %session_id,
            request_id,
            track_namespace = %track_namespace,
            "PUBLISH_NAMESPACE_DONE received"
        );

        let is_origin_client = super::is_origin_client(session_id, forwarder).await;
        // The DONE ending a replaced subscription is ignored: the wider
        // subscription announces the namespace again
        // (`subscribe_namespace::end_replaced_subscriptions`).
        if !is_origin_client
            && request_id.is_some_and(|request_id| {
                cascading_relay_context
                    .inter_relay_connection_manager
                    .namespace_subscriptions
                    .take_superseded(session_id, request_id)
            })
        {
            tracing::debug!(session_id, request_id, %track_namespace, "withdrawal of a replaced namespace subscription ignored");
            return;
        }
        // Announced on a replaced subscription after it was superseded, so
        // whether clients lose it waits for the wider subscription's answer.
        if !is_origin_client
            && cascading_relay_context
                .inter_relay_connection_manager
                .namespace_subscriptions
                .defer_withdrawal(session_id, track_namespace)
        {
            table.unregister_publish_namespace(session_id, request_id, track_namespace);
            tracing::debug!(session_id, request_id, %track_namespace, "withdrawal under a namespace subscription being replaced deferred");
            return;
        }
        let removal = table.unregister_publish_namespace(session_id, request_id, track_namespace);
        // A relay session may withdraw a namespace this relay never registered
        // for it, so only a client's DONE requires its own registration.
        if is_origin_client && removal == PeerRemoval::NotRegistered {
            tracing::warn!(
                session_id = %session_id,
                request_id,
                track_namespace = %track_namespace,
                "PUBLISH_NAMESPACE_DONE for a namespace the session does not publish; ignored"
            );
            return;
        }

        let withdrawn_from = Self::notify_local_subscribers(
            session_id,
            is_origin_client,
            track_namespace,
            table,
            forwarder,
        )
        .await;
        // The end of an earlier request read after the session's later
        // PUBLISH_NAMESPACE of the namespace: the subscribers see the
        // withdrawal and the publication in the order the session sent them.
        for subscriber_session_id in withdrawn_from {
            forwarder
                .publish_namespace(subscriber_session_id, track_namespace.to_string(), || {
                    table.session_publishes_namespace(track_namespace, session_id)
                })
                .await;
        }

        if !is_origin_client {
            Self::end_relay_namespace_subscription(
                session_id,
                track_namespace,
                forwarder,
                cascading_relay_context.inter_relay_connection_manager,
            )
            .await;
        }

        if is_origin_client && removal == PeerRemoval::NoClientsRemain {
            Self::withdraw_namespace_publication(
                track_namespace,
                table,
                forwarder,
                cascading_relay_context,
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
    /// Tells the namespace subscribers the publisher session withdrew it, as
    /// if none of the session's publications of it remained; returns the
    /// subscribers told. The publisher session is one of them when it
    /// subscribes to the namespace, since its own namespace was announced to
    /// it (draft-14 §9.28, draft-18 §10.18). Each subscriber's withdrawal is
    /// decided when it is sent: if another session publishes the namespace
    /// meanwhile, the subscriber keeps it announced.
    pub(crate) async fn notify_local_subscribers(
        publisher_session_id: SessionId,
        publisher_is_client: bool,
        track_namespace: &str,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
    ) -> Vec<SessionId> {
        let mut withdrawn_from = Vec::new();
        for (subscriber_session_id, subscription) in
            table.get_namespace_subscribers(track_namespace, NamespaceInterest::Namespaces)
        {
            // A remaining client publisher keeps the namespace for everyone,
            // a remaining relay publisher for client subscribers (draft-14
            // §9.28); a relay subscriber was told of client publishers only
            // (split horizon).
            let withdraw = || {
                let other_publisher =
                    table.other_namespace_publisher(track_namespace, publisher_session_id);
                match subscription.peer {
                    SessionPeer::Client => other_publisher.is_none(),
                    SessionPeer::Relay => {
                        publisher_is_client && other_publisher != Some(SessionPeer::Client)
                    }
                }
            };
            if forwarder
                .publish_namespace_done(
                    subscriber_session_id,
                    track_namespace.to_string(),
                    withdraw,
                )
                .await
            {
                withdrawn_from.push(subscriber_session_id);
            }
        }
        withdrawn_from
    }

    async fn end_relay_namespace_subscription(
        session_id: SessionId,
        track_namespace: &str,
        forwarder: &ControlMessageForwarder,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        let _serialized = inter_relay_connection_manager
            .namespace_subscriptions
            .lock_session(session_id)
            .await;
        if !inter_relay_connection_manager
            .namespace_subscriptions
            .remove(session_id, track_namespace)
        {
            return;
        }
        if let Err(error) = forwarder
            .unsubscribe_namespace(session_id, track_namespace.to_string())
            .await
        {
            tracing::debug!(%error, session_id, %track_namespace, "failed to unsubscribe the withdrawn namespace");
        }
    }

    /// Withdraws the publisher route unless a client published the namespace
    /// again meanwhile; only once it is withdrawn, tells remote subscriber
    /// relays the namespace is gone, so they can drop their local copy. The
    /// route stays locked until they are told, so a later publisher's
    /// PUBLISH_NAMESPACE reaches them after this DONE.
    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.publish_namespace_done.withdraw_namespace_publication",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    pub(crate) async fn withdraw_namespace_publication(
        track_namespace: &str,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        cascading_relay_context: CascadingRelayContext<'_>,
    ) {
        let _reconciled = match cascading_relay_context
            .namespace_routes
            .reconcile_publisher(track_namespace, table)
            .await
        {
            Ok(reconciled) if reconciled.withdrawn => reconciled,
            Ok(_) => return,
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    track_namespace = %track_namespace,
                    "failed to register namespace route"
                );
                return;
            }
        };

        let relays = match cascading_relay_context
            .route_registry
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

            let Some(session_id) = super::connect_relay(
                cascading_relay_context.inter_relay_connection_manager,
                &relay,
            )
            .await
            else {
                continue;
            };

            forwarder
                .publish_namespace_done(session_id, track_namespace.to_string(), || true)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::modules::test_support::directory_fixtures::unused_inter_relay_client_config;
    use std::{sync::Arc, time::Duration};

    use moqt::{SessionEvent, Version, namespace::PublishNamespaceDoneHandler};

    use super::*;
    use crate::modules::{
        auth::{
            test_support::{app_token, connect_client, spawn_relay_offering},
            verified_token::VerifiedToken,
        },
        cascading::{namespace_routes::NamespaceRoutes, route_registry::NoopRelayRouteRegistry},
        control_plane::sequences::publish_namespace::PublishNamespace,
        data_plane::cache::store::TrackCacheStore,
        directory::track::NamespaceSubscription,
        session::session_repository::NewSession,
        test_support::{
            directory_fixtures::{
                client_subscription, inter_relay_connection_manager, local_publisher_resolver,
                namespaces_and_tracks,
            },
            in_memory_route_registry::InMemoryRouteRegistry,
            mock_session::{
                MockPublishNamespaceHandler, RecordedControlMessages, add_recorded_relay_session,
                detached_session_repository, mock_session_with_transport_stats,
                recorded_session_answering_subscribe, session_repository_with_sessions,
            },
            relay_clients::{next_event, publish_namespace, subscribe_namespace},
        },
    };

    const NAMESPACE: &str = "room/alice";
    const LEAVING_PUBLISHER: SessionId = 1;
    const REMAINING_PUBLISHER: SessionId = 2;
    const NAMESPACE_SUBSCRIBER: SessionId = 3;
    const EARLIER_REQUEST: u64 = 0;
    const LATER_REQUEST: u64 = 2;

    async fn withdraw(
        table: &InMemoryLocalPubSubDirectory,
        publisher_peer: SessionPeer,
    ) -> RecordedControlMessages {
        withdraw_request(table, publisher_peer, None).await
    }

    async fn withdraw_request(
        table: &InMemoryLocalPubSubDirectory,
        publisher_peer: SessionPeer,
        request_id: Option<u64>,
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
            unused_inter_relay_client_config(),
        );
        let forwarder = ControlMessageForwarder { repository };
        forwarder
            .publish_namespace(NAMESPACE_SUBSCRIBER, NAMESPACE.to_string(), || true)
            .await;
        PublishNamespaceDone
            .handle(
                LEAVING_PUBLISHER,
                table,
                &forwarder,
                CascadingRelayContext {
                    route_registry: &NoopRelayRouteRegistry,
                    namespace_routes: &NamespaceRoutes::new(Arc::new(NoopRelayRouteRegistry)),
                    inter_relay_connection_manager: &inter_relay_connection_manager,
                },
                &PublishNamespaceDoneHandler {
                    track_namespace: NAMESPACE.to_string(),
                    track_namespace_tuple: NAMESPACE.split('/').map(str::to_string).collect(),
                    request_id,
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
            table.register_publish_namespace(publisher, 0, NAMESPACE.to_string(), peer);
        }
        table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER,
            "room".to_string(),
            namespaces_and_tracks(SessionPeer::Client),
        );
        table
    }

    /// The leaving publisher published the namespace again before its first
    /// request's end was read.
    fn table_published_twice_by_one_session() -> InMemoryLocalPubSubDirectory {
        let table = table_with_namespace_publishers(&[]);
        for request_id in [EARLIER_REQUEST, LATER_REQUEST] {
            table.register_publish_namespace(
                LEAVING_PUBLISHER,
                request_id,
                NAMESPACE.to_string(),
                SessionPeer::Client,
            );
        }
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
    async fn subscribers_see_the_namespace_withdrawn_and_published_again_when_an_earlier_request_ends_late()
     {
        // Arrange
        let table = table_published_twice_by_one_session();

        // Act
        let recorded = withdraw_request(&table, SessionPeer::Client, Some(EARLIER_REQUEST)).await;

        // Assert: as if the DONE had been read before the later PUBLISH_NAMESPACE
        assert_eq!(
            recorded.namespace_announcements(),
            [
                format!("publish {NAMESPACE}"),
                format!("done {NAMESPACE}"),
                format!("publish {NAMESPACE}"),
            ]
        );
        assert!(table.has_client_namespace_publisher(NAMESPACE));
    }

    #[tokio::test]
    async fn subscribers_are_not_told_of_a_late_request_end_while_another_session_publishes() {
        // Arrange
        let table = table_published_twice_by_one_session();
        table.register_publish_namespace(
            REMAINING_PUBLISHER,
            0,
            NAMESPACE.to_string(),
            SessionPeer::Client,
        );

        // Act
        let recorded = withdraw_request(&table, SessionPeer::Client, Some(EARLIER_REQUEST)).await;

        // Assert
        assert_eq!(
            recorded.namespace_announcements(),
            [format!("publish {NAMESPACE}")]
        );
    }

    #[tokio::test]
    async fn subscribers_are_told_done_only_when_the_later_request_of_the_session_ends() {
        // Arrange
        let table = table_published_twice_by_one_session();
        withdraw_request(&table, SessionPeer::Client, Some(EARLIER_REQUEST)).await;

        // Act
        let recorded = withdraw_request(&table, SessionPeer::Client, Some(LATER_REQUEST)).await;

        // Assert
        assert_eq!(
            recorded.namespace_announcements(),
            [format!("publish {NAMESPACE}"), format!("done {NAMESPACE}")]
        );
        assert!(!table.has_namespace_publishers(NAMESPACE));
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
    async fn a_relay_subscriber_is_told_done_when_the_last_client_publisher_withdraws() {
        // Arrange: the relay publisher left behind was never announced to a relay subscriber
        let table = InMemoryLocalPubSubDirectory::new();
        for (publisher, peer) in [
            (LEAVING_PUBLISHER, SessionPeer::Client),
            (REMAINING_PUBLISHER, SessionPeer::Relay),
        ] {
            table.register_publish_namespace(publisher, 0, NAMESPACE.to_string(), peer);
        }
        table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER,
            NAMESPACE.to_string(),
            NamespaceSubscription {
                interest: NamespaceInterest::Namespaces,
                peer: SessionPeer::Relay,
                forward: true,
            },
        );

        // Act
        let recorded = withdraw(&table, SessionPeer::Client).await;

        // Assert
        assert_eq!(
            recorded.publish_namespace_dones(),
            vec![NAMESPACE.to_string()]
        );
    }

    #[tokio::test]
    async fn a_relay_subscriber_is_not_told_done_for_another_relays_namespace() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(
            LEAVING_PUBLISHER,
            0,
            NAMESPACE.to_string(),
            SessionPeer::Relay,
        );
        table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER,
            NAMESPACE.to_string(),
            namespaces_and_tracks(SessionPeer::Relay),
        );

        // Act
        let recorded = withdraw(&table, SessionPeer::Relay).await;

        // Assert
        assert!(recorded.publish_namespace_dones().is_empty());
    }

    #[tokio::test]
    async fn a_namespace_its_publisher_relay_withdraws_ends_the_subscription_there() {
        // Arrange
        const PUBLISHER_RELAY: SessionId = 4;
        let repository =
            session_repository_with_sessions(Vec::new(), VerifiedToken::full_access()).await;
        let recorded = add_recorded_relay_session(&repository, PUBLISHER_RELAY).await;
        let inter_relay_connection_manager = inter_relay_connection_manager(repository.clone());
        inter_relay_connection_manager
            .namespace_subscriptions
            .insert(PUBLISHER_RELAY, NAMESPACE);
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(
            PUBLISHER_RELAY,
            0,
            NAMESPACE.to_string(),
            SessionPeer::Relay,
        );

        // Act
        PublishNamespaceDone
            .handle(
                PUBLISHER_RELAY,
                &table,
                &ControlMessageForwarder { repository },
                CascadingRelayContext {
                    route_registry: &NoopRelayRouteRegistry,
                    namespace_routes: &NamespaceRoutes::new(Arc::new(NoopRelayRouteRegistry)),
                    inter_relay_connection_manager: &inter_relay_connection_manager,
                },
                &PublishNamespaceDoneHandler {
                    track_namespace: NAMESPACE.to_string(),
                    track_namespace_tuple: NAMESPACE.split('/').map(str::to_string).collect(),
                    request_id: None,
                },
            )
            .await;

        // Assert
        assert_eq!(
            recorded.namespace_subscriptions(),
            vec![format!("unsubscribe {NAMESPACE}")]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_remote_relay_keeps_the_namespace_another_client_publishes_while_the_last_one_withdraws()
     {
        // Arrange
        const REMOTE_RELAY: SessionId = 4;
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        table.register_publish_namespace(
            LEAVING_PUBLISHER,
            0,
            NAMESPACE.to_string(),
            SessionPeer::Client,
        );
        let route_registry = Arc::new(InMemoryRouteRegistry::with_namespace_subscriber_relay(
            "relay-2",
        ));
        let namespace_routes = NamespaceRoutes::new(route_registry.clone());
        drop(
            namespace_routes
                .reconcile_publisher(NAMESPACE, &table)
                .await,
        );
        let repository = session_repository_with_sessions(
            vec![
                (
                    LEAVING_PUBLISHER,
                    mock_session_with_transport_stats(Default::default()),
                ),
                (
                    REMAINING_PUBLISHER,
                    mock_session_with_transport_stats(Default::default()),
                ),
            ],
            VerifiedToken::full_access(),
        )
        .await;
        let remote_relay = add_recorded_relay_session(&repository, REMOTE_RELAY).await;
        let inter_relay_connection_manager = inter_relay_connection_manager(repository.clone());
        inter_relay_connection_manager.record_dialed("relay-2", REMOTE_RELAY);
        let forwarder = ControlMessageForwarder { repository };
        forwarder
            .publish_namespace_to_relay(REMOTE_RELAY, NAMESPACE.to_string())
            .await;
        let context = || CascadingRelayContext {
            route_registry: route_registry.as_ref(),
            namespace_routes: &namespace_routes,
            inter_relay_connection_manager: &inter_relay_connection_manager,
        };
        let done = PublishNamespaceDoneHandler {
            track_namespace: NAMESPACE.to_string(),
            track_namespace_tuple: NAMESPACE.split('/').map(str::to_string).collect(),
            request_id: None,
        };
        let publish = MockPublishNamespaceHandler::new(NAMESPACE);
        let cache_store = Arc::new(TrackCacheStore::new());
        let resolver = Arc::new(local_publisher_resolver());
        let lookup_released = route_registry.hold_next_subscriber_lookup();

        // Act: the next client publishes while the withdrawal looks up the remote relays
        let withdrawal =
            PublishNamespaceDone.handle(LEAVING_PUBLISHER, &table, &forwarder, context(), &done);
        let publication = PublishNamespace.handle(
            REMAINING_PUBLISHER,
            &table,
            &forwarder,
            &cache_store,
            &resolver,
            context(),
            &publish,
        );
        let release = async {
            // Paused time advances only once both handlers wait.
            tokio::time::sleep(Duration::from_secs(1)).await;
            lookup_released.notify_one();
        };
        tokio::join!(withdrawal, publication, release);

        // Assert
        assert_eq!(
            remote_relay.namespace_announcements(),
            [
                format!("publish {NAMESPACE}"),
                format!("done {NAMESPACE}"),
                format!("publish {NAMESPACE}"),
            ]
        );
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
    async fn a_publisher_subscribed_to_its_namespace_is_told_done_and_then_its_next_publisher() {
        for version in [Version::Draft14, Version::Draft18] {
            // Arrange
            let relay = spawn_relay_offering(
                app_token(Some(""), Some("")),
                Arc::new(NoopRelayRouteRegistry),
                &[version],
            );
            let watcher = connect_client(&relay).await;
            let next_publisher = connect_client(&relay).await;
            subscribe_namespace(&watcher, "APP/room").await;
            publish_namespace(&watcher, "APP/room/camera").await;
            let SessionEvent::PublishNamespace(echoed) = next_event(&watcher).await else {
                panic!("{version:?}: expected the watcher's own namespace");
            };
            echoed.ok().await.unwrap();

            // Act: the next publisher waits for the withdrawal, so the relay handles the DONE first
            watcher
                .publisher()
                .publish_namespace_done("APP/room/camera".to_string())
                .await
                .unwrap();
            let withdrawal = next_event(&watcher).await;
            publish_namespace(&next_publisher, "APP/room/camera").await;
            let announcement = next_event(&watcher).await;

            // Assert
            let SessionEvent::PublishNamespaceDone(done) = withdrawal else {
                panic!("{version:?}: expected PUBLISH_NAMESPACE_DONE, got {withdrawal:?}");
            };
            assert_eq!(done.track_namespace, "APP/room/camera");
            let SessionEvent::PublishNamespace(announced) = announcement else {
                panic!("{version:?}: expected PUBLISH_NAMESPACE, got {announcement:?}");
            };
            assert_eq!(announced.track_namespace, "APP/room/camera");
        }
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

    #[tokio::test]
    async fn a_tracks_subscription_is_not_told_done() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(
            LEAVING_PUBLISHER,
            0,
            NAMESPACE.to_string(),
            SessionPeer::Client,
        );
        table.register_subscribe_namespace(
            NAMESPACE_SUBSCRIBER,
            "room".to_string(),
            client_subscription(NamespaceInterest::Tracks),
        );

        // Act
        let recorded = withdraw(&table, SessionPeer::Client).await;

        // Assert: draft-18 §6.1 sends NAMESPACE_DONE for SUBSCRIBE_NAMESPACE only
        assert!(recorded.publish_namespace_dones().is_empty());
    }
}

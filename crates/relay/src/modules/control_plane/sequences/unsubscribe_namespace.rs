use std::collections::HashSet;

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::{RelayInfo, RelayRouteRegistry},
    },
    control_plane::{
        control_message_forwarder::ControlMessageForwarder, sequences::CascadingRelayContext,
    },
    domain::{pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId},
};

pub(crate) struct UnsubscribeNamespace;

impl UnsubscribeNamespace {
    pub(crate) async fn handle(
        &self,
        session_id: SessionId,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        cascading_relay_context: CascadingRelayContext<'_>,
        track_namespace_prefix: &str,
    ) {
        tracing::info!(
            session_id = %session_id,
            track_namespace_prefix = %track_namespace_prefix,
            "UNSUBSCRIBE_NAMESPACE received"
        );

        let no_clients_remain =
            table.unregister_subscribe_namespace(session_id, track_namespace_prefix);
        if !super::is_origin_client(session_id, forwarder).await {
            return;
        }
        if !no_clients_remain {
            return;
        }

        Self::cleanup_empty_namespace_subscription(
            track_namespace_prefix,
            table,
            forwarder,
            cascading_relay_context.route_registry,
            cascading_relay_context.inter_relay_connection_manager,
        )
        .await;
    }

    #[tracing::instrument(
        level = "debug",
        name = "relay.sequence.unsubscribe_namespace.cleanup_empty_namespace_subscription",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) async fn cleanup_empty_namespace_subscription(
        track_namespace_prefix: &str,
        table: &InMemoryLocalPubSubDirectory,
        forwarder: &ControlMessageForwarder,
        route_registry: &dyn RelayRouteRegistry,
        inter_relay_connection_manager: &InterRelayConnectionManager,
    ) {
        table.purge_relay_publish_namespaces(track_namespace_prefix);

        if let Err(err) = route_registry
            .unregister_namespace_subscriber(track_namespace_prefix)
            .await
        {
            tracing::warn!(
                error = ?err,
                track_namespace_prefix = %track_namespace_prefix,
                "failed to unregister namespace subscription route"
            );
        }

        let routes = Self::find_publisher_relays(track_namespace_prefix, route_registry).await;
        for route in routes {
            let Some(session_id) =
                super::connect_relay(inter_relay_connection_manager, &route).await
            else {
                continue;
            };

            if let Err(err) = forwarder
                .unsubscribe_namespace(session_id, track_namespace_prefix.to_string())
                .await
            {
                tracing::warn!(
                    error = ?err,
                    relay_id = %route.relay_id,
                    session_id = session_id,
                    track_namespace_prefix = %track_namespace_prefix,
                    "failed to forward UNSUBSCRIBE_NAMESPACE to upstream relay"
                );
            }
        }
    }

    async fn find_publisher_relays(
        track_namespace_prefix: &str,
        route_registry: &dyn RelayRouteRegistry,
    ) -> Vec<RelayInfo> {
        let namespace_routes = match route_registry
            .find_namespace_publishers_by_prefix(track_namespace_prefix)
            .await
        {
            Ok(routes) => routes,
            Err(err) => {
                tracing::warn!(
                    error = ?err,
                    track_namespace_prefix = %track_namespace_prefix,
                    "failed to find namespace routes for UNSUBSCRIBE_NAMESPACE"
                );
                return Vec::new();
            }
        };

        let mut relay_ids = HashSet::new();
        let mut relays = Vec::new();
        for namespace_route in namespace_routes {
            let publisher_relays = match route_registry
                .find_active_namespace_publishers(&namespace_route.track_namespace)
                .await
            {
                Ok(publisher_relays) => publisher_relays,
                Err(err) => {
                    tracing::warn!(
                        error = ?err,
                        track_namespace = %namespace_route.track_namespace,
                        "failed to find publisher relays for UNSUBSCRIBE_NAMESPACE"
                    );
                    continue;
                }
            };

            for relay in publisher_relays {
                if relay_ids.insert(relay.relay_id.clone()) {
                    relays.push(relay);
                }
            }
        }

        relays
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use moqt::SessionEvent;

    use crate::modules::{
        auth::test_support::{
            app_token, connect_client, spawn_relay, spawn_relay_and_connect_client,
        },
        test_support::{
            relay_clients::{next_event, publish_namespace, subscribe_namespace},
            stub_route_registry::{PublisherLookup, StubRouteRegistry},
        },
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn namespaces_announced_after_unsubscribe_namespace_are_not_forwarded() {
        // Arrange: the relay answers "APP/lobby" only after it handled the UNSUBSCRIBE_NAMESPACE
        let (relay, watcher) = spawn_relay_and_connect_client(app_token(Some(""), Some(""))).await;
        let announcer = connect_client(&relay).await;
        subscribe_namespace(&watcher, "APP/room").await;
        watcher
            .subscriber()
            .unsubscribe_namespace("APP/room".to_string())
            .await
            .unwrap();
        subscribe_namespace(&watcher, "APP/lobby").await;

        // Act
        publish_namespace(&announcer, "APP/room/y").await;
        publish_namespace(&announcer, "APP/lobby/z").await;
        let event = next_event(&watcher).await;

        // Assert: "APP/room/y" would have arrived first had it been forwarded
        let SessionEvent::PublishNamespace(announced) = event else {
            panic!("expected PUBLISH_NAMESPACE, got {event:?}");
        };
        assert_eq!(announced.track_namespace, "APP/lobby/z");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_last_unsubscribe_namespace_of_a_prefix_removes_its_route() {
        // Arrange
        let route_registry = Arc::new(StubRouteRegistry::new(PublisherLookup::NotFound));
        let relay = spawn_relay(app_token(Some(""), Some("")), route_registry.clone()).await;
        let watcher = connect_client(&relay).await;
        subscribe_namespace(&watcher, "APP/room").await;

        // Act
        watcher
            .subscriber()
            .unsubscribe_namespace("APP/room".to_string())
            .await
            .unwrap();
        subscribe_namespace(&watcher, "APP/lobby").await;

        // Assert
        assert_eq!(
            route_registry.namespace_subscriber_calls(),
            [
                "register APP/room",
                "unregister APP/room",
                "register APP/lobby"
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsubscribe_namespace_keeps_the_route_while_another_client_watches_the_prefix() {
        // Arrange
        let route_registry = Arc::new(StubRouteRegistry::new(PublisherLookup::NotFound));
        let relay = spawn_relay(app_token(Some(""), Some("")), route_registry.clone()).await;
        let leaving = connect_client(&relay).await;
        let staying = connect_client(&relay).await;
        subscribe_namespace(&leaving, "APP/room").await;
        subscribe_namespace(&staying, "APP/room").await;

        // Act
        leaving
            .subscriber()
            .unsubscribe_namespace("APP/room".to_string())
            .await
            .unwrap();
        subscribe_namespace(&leaving, "APP/lobby").await;

        // Assert
        assert_eq!(
            route_registry.namespace_subscriber_calls(),
            ["register APP/room", "register APP/lobby"]
        );
    }
}

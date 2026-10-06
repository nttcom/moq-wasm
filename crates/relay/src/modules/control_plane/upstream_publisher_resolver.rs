use std::{cmp::Reverse, sync::Arc};

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::RelayRouteRegistry, watched_namespace_routes::WatchedNamespaceRoutes,
    },
    domain::{
        pub_sub_directory::{InMemoryLocalPubSubDirectory, entry::UpstreamSubscriptionKey},
        session_peer::SessionPeer,
    },
};

pub(crate) struct UpstreamPublisherResolver {
    route_registry: Arc<dyn RelayRouteRegistry>,
    inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    watched_namespace_routes: Arc<WatchedNamespaceRoutes>,
}

impl UpstreamPublisherResolver {
    pub(crate) fn new(
        route_registry: Arc<dyn RelayRouteRegistry>,
        inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    ) -> Self {
        Self {
            watched_namespace_routes: Arc::new(WatchedNamespaceRoutes::new(route_registry.clone())),
            route_registry,
            inter_relay_connection_manager,
        }
    }

    pub(crate) fn watched_namespace_routes(&self) -> Arc<WatchedNamespaceRoutes> {
        self.watched_namespace_routes.clone()
    }

    /// Registering before resolving means a relay announcing the namespace
    /// afterwards finds this relay among its namespace subscribers.
    pub(crate) async fn watch_namespace(&self, track_namespace: &str) {
        self.watched_namespace_routes.watch(track_namespace).await;
    }

    /// Every publisher of the track: the local client publishers newest first
    /// (session ids grow with time), then, for a client requester, every
    /// remote relay publishing the namespace. A relay requester is served
    /// from local client publishers only, so a request never travels from
    /// relay to relay and subscriptions cannot form a loop.
    #[tracing::instrument(
        level = "info",
        name = "relay.upstream_publisher_resolver.resolve",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn resolve(
        &self,
        table: &InMemoryLocalPubSubDirectory,
        track_namespace: &str,
        track_name: &str,
        requester: SessionPeer,
    ) -> Vec<UpstreamSubscriptionKey> {
        let mut publishers =
            table.find_upstream_publishers(track_namespace, track_name, SessionPeer::Client);
        publishers.sort_by_key(|publisher| Reverse(publisher.publisher_session_id));
        if requester == SessionPeer::Relay {
            return publishers;
        }

        let relays = match self
            .route_registry
            .find_active_namespace_publishers(track_namespace)
            .await
        {
            Ok(relays) => relays,
            Err(err) => {
                tracing::warn!(
                    ?err,
                    %track_namespace,
                    "failed to find publisher relays; falling back to relay sessions that announced the namespace"
                );
                let mut relay_publishers =
                    table.find_upstream_publishers(track_namespace, track_name, SessionPeer::Relay);
                relay_publishers.sort_by_key(|publisher| Reverse(publisher.publisher_session_id));
                publishers.extend(relay_publishers);
                return publishers;
            }
        };
        for relay in relays {
            match self
                .inter_relay_connection_manager
                .get_or_connect(&relay)
                .await
            {
                Ok(publisher_session_id) => {
                    tracing::info!(
                        relay_id = %relay.relay_id,
                        publisher_session_id = publisher_session_id,
                        track_namespace = %track_namespace,
                        track_name = %track_name,
                        "resolved remote upstream publisher"
                    );
                    publishers.push(UpstreamSubscriptionKey {
                        publisher_session_id,
                        track_namespace: track_namespace.to_string(),
                        track_name: track_name.to_string(),
                    });
                }
                Err(err) => tracing::warn!(
                    ?err,
                    relay_id = %relay.relay_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to connect remote upstream publisher"
                ),
            }
        }
        publishers
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        cascading::route_registry::{NamespaceRoute, RegisterRouteError, RelayInfo},
        domain::{
            pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId,
            session_peer::SessionPeer,
        },
        session::session_repository::SessionRepository,
    };

    enum PublisherLookup {
        NotFound,
        Fails,
        MustNotBeCalled,
    }

    struct StubRouteRegistry {
        lookup: PublisherLookup,
    }

    #[async_trait::async_trait]
    impl RelayRouteRegistry for StubRouteRegistry {
        async fn register_namespace_publisher(
            &self,
            _track_namespace: &str,
        ) -> Result<(), RegisterRouteError> {
            unimplemented!("not used in resolver tests")
        }

        async fn register_namespace_subscriber(
            &self,
            _track_namespace_prefix: &str,
        ) -> Result<(), RegisterRouteError> {
            unimplemented!("not used in resolver tests")
        }

        async fn find_active_namespace_publishers(
            &self,
            _track_namespace: &str,
        ) -> anyhow::Result<Vec<RelayInfo>> {
            match self.lookup {
                PublisherLookup::NotFound => Ok(Vec::new()),
                PublisherLookup::Fails => Err(anyhow::anyhow!("route registry down")),
                PublisherLookup::MustNotBeCalled => {
                    panic!("a relay requester must not be routed to another relay")
                }
            }
        }

        async fn register_watched_namespace(&self, _track_namespace: &str) -> anyhow::Result<()> {
            unimplemented!("not used in resolver tests")
        }

        async fn unregister_watched_namespace(&self, _track_namespace: &str) -> anyhow::Result<()> {
            unimplemented!("not used in resolver tests")
        }

        async fn find_namespace_publishers_by_prefix(
            &self,
            _track_namespace_prefix: &str,
        ) -> anyhow::Result<Vec<NamespaceRoute>> {
            unimplemented!("not used in resolver tests")
        }

        async fn unregister_namespace_publisher(
            &self,
            _track_namespace: &str,
        ) -> anyhow::Result<()> {
            unimplemented!("not used in resolver tests")
        }

        async fn unregister_namespace_subscriber(
            &self,
            _track_namespace_prefix: &str,
        ) -> anyhow::Result<()> {
            unimplemented!("not used in resolver tests")
        }

        async fn find_namespace_subscribers(
            &self,
            _track_namespace: &str,
        ) -> anyhow::Result<Vec<RelayInfo>> {
            unimplemented!("not used in resolver tests")
        }
    }

    fn make_resolver(lookup: PublisherLookup) -> UpstreamPublisherResolver {
        let repository = Arc::new(tokio::sync::Mutex::new(SessionRepository::new()));
        let (session_event_sender, _session_event_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        UpstreamPublisherResolver::new(
            Arc::new(StubRouteRegistry { lookup }),
            Arc::new(InterRelayConnectionManager::new(
                repository,
                session_event_sender,
                "unused-relay-token".to_string(),
            )),
        )
    }

    fn table_with_publishers() -> InMemoryLocalPubSubDirectory {
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(3, "ns".to_string(), SessionPeer::Client);
        table.register_publish_namespace(5, "ns".to_string(), SessionPeer::Client);
        table.register_publish_namespace(7, "ns".to_string(), SessionPeer::Relay);
        table
    }

    fn session_ids(publishers: &[UpstreamSubscriptionKey]) -> Vec<SessionId> {
        publishers
            .iter()
            .map(|publisher| publisher.publisher_session_id)
            .collect()
    }

    #[tokio::test]
    async fn resolves_every_local_client_publisher_newest_first() {
        // Arrange
        let table = table_with_publishers();
        let resolver = make_resolver(PublisherLookup::NotFound);

        // Act
        let resolved = resolver
            .resolve(&table, "ns", "track", SessionPeer::Client)
            .await;

        // Assert
        assert_eq!(session_ids(&resolved), vec![5, 3]);
        assert!(
            resolved
                .iter()
                .all(|publisher| publisher.track_namespace == "ns"
                    && publisher.track_name == "track")
        );
    }

    #[tokio::test]
    async fn a_relay_requester_is_served_from_local_client_publishers_only() {
        // Arrange
        let table = table_with_publishers();
        let resolver = make_resolver(PublisherLookup::MustNotBeCalled);

        // Act
        let resolved = resolver
            .resolve(&table, "ns", "track", SessionPeer::Relay)
            .await;

        // Assert
        assert_eq!(session_ids(&resolved), vec![5, 3]);
    }

    #[tokio::test]
    async fn resolves_nothing_when_no_publisher_anywhere() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let resolver = make_resolver(PublisherLookup::NotFound);

        // Act
        let resolved = resolver
            .resolve(&table, "ns", "track", SessionPeer::Client)
            .await;

        // Assert
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn falls_back_to_announcing_relay_sessions_when_the_route_registry_fails() {
        // Arrange
        let table = table_with_publishers();
        let resolver = make_resolver(PublisherLookup::Fails);

        // Act
        let resolved = resolver
            .resolve(&table, "ns", "track", SessionPeer::Client)
            .await;

        // Assert
        assert_eq!(session_ids(&resolved), vec![5, 3, 7]);
    }
}

use std::{cmp::Reverse, sync::Arc};

use crate::modules::{
    cascading::{
        inter_relay_connection_manager::InterRelayConnectionManager,
        route_registry::{RelayInfo, RelayRouteRegistry},
        watched_namespace_routes::WatchedNamespaceRoutes,
    },
    domain::{
        pub_sub_directory::InMemoryLocalPubSubDirectory, session_id::SessionId,
        session_peer::SessionPeer,
    },
};

/// A remote relay is only dialled by whoever sends it the request, so a relay
/// that does not answer delays nothing else.
#[derive(Clone, Debug)]
pub(crate) enum UpstreamPublisher {
    Session(SessionId),
    Relay(RelayInfo),
}

pub(crate) struct UpstreamPublisherResolver {
    route_registry: Arc<dyn RelayRouteRegistry>,
    inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    pub(crate) watched_namespace_routes: Arc<WatchedNamespaceRoutes>,
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
    ) -> Vec<UpstreamPublisher> {
        let mut local_publishers: Vec<SessionId> = table
            .find_upstream_publishers(track_namespace, track_name)
            .into_iter()
            .map(|publisher| publisher.publisher_session_id)
            .collect();
        local_publishers.sort_by_key(|publisher_session_id| Reverse(*publisher_session_id));
        let mut publishers: Vec<UpstreamPublisher> = local_publishers
            .into_iter()
            .map(UpstreamPublisher::Session)
            .collect();
        if requester == SessionPeer::Relay {
            return publishers;
        }
        match self
            .route_registry
            .find_active_namespace_publishers(track_namespace)
            .await
        {
            Ok(relays) => publishers.extend(relays.into_iter().map(UpstreamPublisher::Relay)),
            Err(err) => tracing::warn!(?err, %track_namespace, "failed to find publisher relays"),
        }
        publishers
    }

    pub(crate) async fn session_of(&self, publisher: &UpstreamPublisher) -> Option<SessionId> {
        match publisher {
            UpstreamPublisher::Session(publisher_session_id) => Some(*publisher_session_id),
            UpstreamPublisher::Relay(relay) => self
                .inter_relay_connection_manager
                .get_or_connect(relay)
                .await
                .inspect_err(|err| {
                    tracing::warn!(?err, relay_id = %relay.relay_id, "failed to connect a publisher relay")
                })
                .ok(),
        }
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
        async fn register_namespace_publisher(&self, _track_namespace: &str) -> anyhow::Result<()> {
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

    fn session_ids(publishers: &[UpstreamPublisher]) -> Vec<SessionId> {
        publishers
            .iter()
            .filter_map(|publisher| match publisher {
                UpstreamPublisher::Session(publisher_session_id) => Some(*publisher_session_id),
                UpstreamPublisher::Relay(_) => None,
            })
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
    async fn a_failing_route_registry_leaves_the_local_publishers() {
        // Arrange
        let table = table_with_publishers();
        let resolver = make_resolver(PublisherLookup::Fails);

        // Act
        let resolved = resolver
            .resolve(&table, "ns", "track", SessionPeer::Client)
            .await;

        // Assert
        assert_eq!(session_ids(&resolved), vec![5, 3]);
    }
}

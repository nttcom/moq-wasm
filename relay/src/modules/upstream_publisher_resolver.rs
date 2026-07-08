use std::sync::Arc;

use crate::modules::{
    inter_relay::InterRelayConnectionManager,
    route_registry::RelayRouteRegistry,
    sequences::tables::table::{LocalPubSubDirectory, UpstreamSubscriptionKey},
};

pub(crate) struct UpstreamPublisherResolver {
    route_registry: Arc<dyn RelayRouteRegistry>,
    inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
}

impl UpstreamPublisherResolver {
    pub(crate) fn new(
        route_registry: Arc<dyn RelayRouteRegistry>,
        inter_relay_connection_manager: Arc<InterRelayConnectionManager>,
    ) -> Self {
        Self {
            route_registry,
            inter_relay_connection_manager,
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.upstream_publisher_resolver.resolve",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn resolve(
        &self,
        table: &dyn LocalPubSubDirectory,
        track_namespace: &str,
        track_name: &str,
    ) -> anyhow::Result<Option<UpstreamSubscriptionKey>> {
        if let Some(local_publisher) = self
            .find_local_publisher(table, track_namespace, track_name)
            .await
        {
            return Ok(Some(local_publisher));
        }

        self.find_remote_publisher(track_namespace, track_name)
            .await
    }

    async fn find_local_publisher(
        &self,
        table: &dyn LocalPubSubDirectory,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<UpstreamSubscriptionKey> {
        table
            .find_upstream_publishers(track_namespace, track_name)
            .await
            .into_iter()
            .min_by_key(|publisher| publisher.publisher_session_id)
    }

    async fn find_remote_publisher(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> anyhow::Result<Option<UpstreamSubscriptionKey>> {
        let Some(relay) = self
            .route_registry
            .find_active_namespace_publisher(track_namespace)
            .await?
        else {
            return Ok(None);
        };

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
                Ok(Some(UpstreamSubscriptionKey {
                    publisher_session_id,
                    track_namespace: track_namespace.to_string(),
                    track_name: track_name.to_string(),
                }))
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    relay_id = %relay.relay_id,
                    track_namespace = %track_namespace,
                    track_name = %track_name,
                    "failed to connect remote upstream publisher"
                );
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        route_registry::{
            NamespaceRoute, RegisterNamespacePublisherError, RegisterNamespaceSubscriberError,
            RelayInfo, RouteStatus,
        },
        sequences::tables::{hashmap_table::InMemoryLocalPubSubDirectory, table::PeerKind},
        session_repository::SessionRepository,
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
            _status: RouteStatus,
        ) -> Result<(), RegisterNamespacePublisherError> {
            unimplemented!("not used in resolver tests")
        }

        async fn register_namespace_subscriber(
            &self,
            _track_namespace_prefix: &str,
            _status: RouteStatus,
        ) -> Result<(), RegisterNamespaceSubscriberError> {
            unimplemented!("not used in resolver tests")
        }

        async fn find_active_namespace_publisher(
            &self,
            _track_namespace: &str,
        ) -> anyhow::Result<Option<RelayInfo>> {
            match self.lookup {
                PublisherLookup::NotFound => Ok(None),
                PublisherLookup::Fails => Err(anyhow::anyhow!("route registry down")),
                PublisherLookup::MustNotBeCalled => {
                    panic!("route registry must not be consulted when a local publisher exists")
                }
            }
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
            )),
        )
    }

    #[tokio::test]
    async fn prefers_local_publisher_and_picks_min_session_id() {
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(5, "ns".to_string(), PeerKind::Client);
        table.register_publish_namespace(3, "ns".to_string(), PeerKind::Client);
        let resolver = make_resolver(PublisherLookup::MustNotBeCalled);

        let resolved = resolver
            .resolve(&table, "ns", "track")
            .await
            .expect("resolve should succeed")
            .expect("local publisher should be found");

        assert_eq!(resolved.publisher_session_id, 3);
        assert_eq!(resolved.track_namespace, "ns");
        assert_eq!(resolved.track_name, "track");
    }

    #[tokio::test]
    async fn returns_none_when_no_publisher_anywhere() {
        let table = InMemoryLocalPubSubDirectory::new();
        let resolver = make_resolver(PublisherLookup::NotFound);

        let resolved = resolver
            .resolve(&table, "ns", "track")
            .await
            .expect("resolve should succeed");

        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn propagates_route_registry_error() {
        let table = InMemoryLocalPubSubDirectory::new();
        let resolver = make_resolver(PublisherLookup::Fails);

        let result = resolver.resolve(&table, "ns", "track").await;

        assert!(result.is_err());
    }
}

use std::sync::Arc;

use dashmap::DashMap;

use crate::modules::{
    cascading::route_registry::RelayRouteRegistry, directory::InMemoryLocalPubSubDirectory,
};

/// One lock per route key, held across the route registry call; the value is
/// whether this relay holds the route.
#[derive(Default)]
struct HeldRoutes(DashMap<String, Arc<tokio::sync::Mutex<bool>>>);

impl HeldRoutes {
    async fn lock(&self, key: &str) -> tokio::sync::OwnedMutexGuard<bool> {
        let route = self.0.entry(key.to_string()).or_default().clone();
        route.lock_owned().await
    }

    /// Drops the lock of a route this relay does not hold once no caller
    /// waits for it. A caller clones the lock only under the map's shard
    /// lock, which `remove_if` holds, so no caller is left with a lock the
    /// map forgot.
    fn forget_unheld(&self, key: &str) {
        self.0.remove_if(key, |_, route| {
            Arc::strong_count(route) == 1 && route.try_lock().is_ok_and(|held| !*held)
        });
    }
}

/// The namespace routes this relay holds in the route registry. After each
/// change of a namespace's client publishers in the directory, `reconcile`
/// reads them again under the namespace's lock and registers or withdraws
/// the route to match, so two changes crossing each other leave the route
/// the directory asks for.
pub(crate) struct NamespaceRoutes {
    route_registry: Arc<dyn RelayRouteRegistry>,
    publishers: HeldRoutes,
}

impl NamespaceRoutes {
    pub(crate) fn new(route_registry: Arc<dyn RelayRouteRegistry>) -> Self {
        Self {
            route_registry,
            publishers: HeldRoutes::default(),
        }
    }

    /// Registers the namespace's publisher route while a client publishes the
    /// namespace and withdraws it once none does. Fails only when the route
    /// could not be registered; a failed withdrawal is logged. Returns whether
    /// it withdrew the route, and the route
    /// expires as the heartbeat no longer refreshes it.
    pub(crate) async fn reconcile_publisher(
        &self,
        track_namespace: &str,
        table: &InMemoryLocalPubSubDirectory,
    ) -> anyhow::Result<bool> {
        let mut held = self.publishers.lock(track_namespace).await;
        let wanted = table.has_client_namespace_publisher(track_namespace);
        let withdrawn = match (wanted, *held) {
            (true, false) => self
                .route_registry
                .register_namespace_publisher(track_namespace)
                .await
                .map(|()| false),
            (false, true) => {
                if let Err(error) = self
                    .route_registry
                    .unregister_namespace_publisher(track_namespace)
                    .await
                {
                    tracing::warn!(?error, %track_namespace, "failed to unregister namespace publisher route");
                }
                Ok(true)
            }
            _ => Ok(false),
        };
        if withdrawn.is_ok() {
            *held = wanted;
        }
        drop(held);
        self.publishers.forget_unheld(track_namespace);
        withdrawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{
        domain::session_peer::SessionPeer,
        test_support::in_memory_route_registry::InMemoryRouteRegistry,
    };

    #[tokio::test]
    async fn the_route_follows_the_client_publishers_of_the_namespace() {
        // Arrange
        let registry = Arc::new(InMemoryRouteRegistry::default());
        let routes = NamespaceRoutes::new(registry.clone());
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, 0, "ns".to_string(), SessionPeer::Client);
        table.register_publish_namespace(2, 0, "ns".to_string(), SessionPeer::Client);

        // Act
        routes.reconcile_publisher("ns", &table).await.unwrap();
        let registered = registry.routes().namespace_publishers;
        table.unregister_publish_namespace(1, None, "ns");
        let one_left = routes.reconcile_publisher("ns", &table).await.unwrap();
        table.unregister_publish_namespace(2, None, "ns");
        let none_left = routes.reconcile_publisher("ns", &table).await.unwrap();

        // Assert
        assert!(registered.contains("ns"));
        assert_eq!([one_left, none_left], [false, true]);
        assert!(registry.routes().namespace_publishers.is_empty());
        assert!(routes.publishers.0.is_empty());
    }

    #[tokio::test]
    async fn a_relay_publisher_takes_no_route() {
        // Arrange
        let registry = Arc::new(InMemoryRouteRegistry::default());
        let routes = NamespaceRoutes::new(registry.clone());
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, 0, "ns".to_string(), SessionPeer::Relay);

        // Act
        routes.reconcile_publisher("ns", &table).await.unwrap();

        // Assert
        assert!(registry.routes().namespace_publishers.is_empty());
    }
}

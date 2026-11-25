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

#[derive(Clone, Copy)]
enum Route<'a> {
    Publisher(&'a str),
    Subscriber(&'a str),
}

impl Route<'_> {
    fn is_wanted(self, table: &InMemoryLocalPubSubDirectory) -> bool {
        match self {
            Self::Publisher(track_namespace) => {
                table.has_client_namespace_publisher(track_namespace)
            }
            Self::Subscriber(track_namespace_prefix) => {
                table.has_client_namespace_subscriber(track_namespace_prefix)
            }
        }
    }

    async fn register(self, route_registry: &dyn RelayRouteRegistry) -> anyhow::Result<()> {
        match self {
            Self::Publisher(track_namespace) => {
                route_registry
                    .register_namespace_publisher(track_namespace)
                    .await
            }
            Self::Subscriber(track_namespace_prefix) => {
                route_registry
                    .register_namespace_subscriber(track_namespace_prefix)
                    .await
            }
        }
    }

    /// Logs a failure: the route expires as the heartbeat no longer
    /// refreshes it.
    async fn unregister(self, route_registry: &dyn RelayRouteRegistry) {
        match self {
            Self::Publisher(track_namespace) => {
                if let Err(error) = route_registry
                    .unregister_namespace_publisher(track_namespace)
                    .await
                {
                    tracing::warn!(?error, %track_namespace, "failed to unregister namespace publisher route");
                }
            }
            Self::Subscriber(track_namespace_prefix) => {
                if let Err(error) = route_registry
                    .unregister_namespace_subscriber(track_namespace_prefix)
                    .await
                {
                    tracing::warn!(?error, %track_namespace_prefix, "failed to unregister namespace subscriber route");
                }
            }
        }
    }
}

/// The namespace routes this relay holds in the route registry. After each
/// change of a namespace's client publishers or a prefix's client subscribers
/// in the directory, a reconcile reads them again under the route's lock and
/// registers or withdraws the route to match, so two changes crossing each
/// other leave the route the directory asks for.
pub(crate) struct NamespaceRoutes {
    route_registry: Arc<dyn RelayRouteRegistry>,
    publishers: HeldRoutes,
    subscribers: HeldRoutes,
}

impl NamespaceRoutes {
    pub(crate) fn new(route_registry: Arc<dyn RelayRouteRegistry>) -> Self {
        Self {
            route_registry,
            publishers: HeldRoutes::default(),
            subscribers: HeldRoutes::default(),
        }
    }

    /// Registers the namespace's publisher route while a client publishes the
    /// namespace and withdraws it once none does. Fails only when the route
    /// could not be registered; a failed withdrawal is logged. Returns whether
    /// it withdrew the route.
    pub(crate) async fn reconcile_publisher(
        &self,
        track_namespace: &str,
        table: &InMemoryLocalPubSubDirectory,
    ) -> anyhow::Result<bool> {
        self.reconcile(&self.publishers, Route::Publisher(track_namespace), table)
            .await
    }

    /// `reconcile_publisher` for the prefix's subscriber route, held while a
    /// client subscribes to the prefix.
    pub(crate) async fn reconcile_subscriber(
        &self,
        track_namespace_prefix: &str,
        table: &InMemoryLocalPubSubDirectory,
    ) -> anyhow::Result<bool> {
        self.reconcile(
            &self.subscribers,
            Route::Subscriber(track_namespace_prefix),
            table,
        )
        .await
    }

    async fn reconcile(
        &self,
        held_routes: &HeldRoutes,
        route: Route<'_>,
        table: &InMemoryLocalPubSubDirectory,
    ) -> anyhow::Result<bool> {
        let (Route::Publisher(key) | Route::Subscriber(key)) = route;
        let mut held = held_routes.lock(key).await;
        let wanted = route.is_wanted(table);
        let withdrawn = match (wanted, *held) {
            (true, false) => route
                .register(self.route_registry.as_ref())
                .await
                .map(|()| false),
            (false, true) => {
                route.unregister(self.route_registry.as_ref()).await;
                Ok(true)
            }
            _ => Ok(false),
        };
        if withdrawn.is_ok() {
            *held = wanted;
        }
        drop(held);
        held_routes.forget_unheld(key);
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

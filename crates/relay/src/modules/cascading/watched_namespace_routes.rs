use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::modules::{
    cascading::route_registry::RelayRouteRegistry, domain::track_key::TrackNamespace,
};

/// The watched-namespace routes this relay holds in the route registry.
/// `watch` registers one as soon as a client asks for a track; `reconcile`
/// drops a route only after two passes in a row found no client watching the
/// namespace, so a route registered just before its track exists survives.
pub(crate) struct WatchedNamespaceRoutes {
    route_registry: Arc<dyn RelayRouteRegistry>,
    unwatched_at_last_pass: tokio::sync::Mutex<HashMap<TrackNamespace, bool>>,
}

impl WatchedNamespaceRoutes {
    pub(crate) fn new(route_registry: Arc<dyn RelayRouteRegistry>) -> Self {
        Self {
            route_registry,
            unwatched_at_last_pass: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn watch(&self, track_namespace: &str) {
        let newly_watched = self
            .unwatched_at_last_pass
            .lock()
            .await
            .insert(track_namespace.to_string(), false)
            .is_none();
        if !newly_watched {
            return;
        }
        if let Err(err) = self
            .route_registry
            .register_watched_namespace(track_namespace)
            .await
        {
            tracing::warn!(?err, %track_namespace, "failed to register the watched namespace");
            self.unwatched_at_last_pass
                .lock()
                .await
                .remove(track_namespace);
        }
    }

    pub(crate) async fn reconcile(&self, watched: &HashSet<TrackNamespace>) {
        for track_namespace in watched {
            self.watch(track_namespace).await;
        }

        let released: Vec<TrackNamespace> = {
            let mut registered = self.unwatched_at_last_pass.lock().await;
            let mut released = Vec::new();
            for (track_namespace, unwatched_at_last_pass) in registered.iter_mut() {
                if watched.contains(track_namespace) {
                    *unwatched_at_last_pass = false;
                } else if *unwatched_at_last_pass {
                    released.push(track_namespace.clone());
                } else {
                    *unwatched_at_last_pass = true;
                }
            }
            for track_namespace in &released {
                registered.remove(track_namespace);
            }
            released
        };
        for track_namespace in released {
            if let Err(err) = self
                .route_registry
                .unregister_watched_namespace(&track_namespace)
                .await
            {
                tracing::warn!(?err, %track_namespace, "failed to unregister the watched namespace");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::stub_route_registry::{PublisherLookup, StubRouteRegistry};

    fn recording_registry() -> Arc<StubRouteRegistry> {
        Arc::new(StubRouteRegistry::new(PublisherLookup::NotFound))
    }

    fn watched(track_namespaces: &[&str]) -> HashSet<TrackNamespace> {
        track_namespaces.iter().map(ToString::to_string).collect()
    }

    #[tokio::test]
    async fn a_namespace_is_unregistered_after_two_passes_without_a_watcher() {
        // Arrange
        let registry = recording_registry();
        let routes = WatchedNamespaceRoutes::new(registry.clone());
        routes.watch("ns").await;

        // Act
        routes.reconcile(&watched(&[])).await;
        let after_first_pass = registry.watched_namespace_calls();
        routes.reconcile(&watched(&[])).await;

        // Assert
        assert_eq!(after_first_pass, vec!["register ns"]);
        assert_eq!(
            registry.watched_namespace_calls(),
            vec!["register ns", "unregister ns"]
        );
    }

    #[tokio::test]
    async fn a_watcher_seen_between_passes_keeps_the_namespace_registered() {
        // Arrange
        let registry = recording_registry();
        let routes = WatchedNamespaceRoutes::new(registry.clone());
        routes.watch("ns").await;

        // Act
        routes.reconcile(&watched(&[])).await;
        routes.reconcile(&watched(&["ns"])).await;
        routes.reconcile(&watched(&[])).await;

        // Assert
        assert_eq!(registry.watched_namespace_calls(), vec!["register ns"]);
    }

    #[tokio::test]
    async fn a_watched_namespace_found_by_a_pass_is_registered() {
        // Arrange
        let registry = recording_registry();
        let routes = WatchedNamespaceRoutes::new(registry.clone());

        // Act
        routes.reconcile(&watched(&["ns"])).await;

        // Assert
        assert_eq!(registry.watched_namespace_calls(), vec!["register ns"]);
    }
}

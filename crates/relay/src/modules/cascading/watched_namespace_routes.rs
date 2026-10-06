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
        let mut registered = self.unwatched_at_last_pass.lock().await;
        if registered.contains_key(track_namespace) {
            registered.insert(track_namespace.to_string(), false);
            return;
        }
        match self
            .route_registry
            .register_watched_namespace(track_namespace)
            .await
        {
            Ok(()) => {
                registered.insert(track_namespace.to_string(), false);
            }
            Err(err) => {
                tracing::warn!(?err, %track_namespace, "failed to register the watched namespace")
            }
        }
    }

    pub(crate) async fn reconcile(&self, watched: &HashSet<TrackNamespace>) {
        let newly_watched: Vec<_> = {
            let registered = self.unwatched_at_last_pass.lock().await;
            watched
                .iter()
                .filter(|track_namespace| !registered.contains_key(*track_namespace))
                .cloned()
                .collect()
        };
        for track_namespace in newly_watched {
            self.watch(&track_namespace).await;
        }

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
        for track_namespace in released {
            registered.remove(&track_namespace);
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
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::modules::cascading::route_registry::{
        NamespaceRoute, RegisterRouteError, RelayInfo,
    };

    #[derive(Default)]
    struct RecordingRouteRegistry {
        calls: Mutex<Vec<String>>,
    }

    impl RecordingRouteRegistry {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl RelayRouteRegistry for RecordingRouteRegistry {
        async fn register_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("register {track_namespace}"));
            Ok(())
        }

        async fn unregister_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("unregister {track_namespace}"));
            Ok(())
        }

        async fn register_namespace_publisher(
            &self,
            _track_namespace: &str,
        ) -> Result<(), RegisterRouteError> {
            unimplemented!("not used by watched namespace routes")
        }

        async fn register_namespace_subscriber(
            &self,
            _track_namespace_prefix: &str,
        ) -> Result<(), RegisterRouteError> {
            unimplemented!("not used by watched namespace routes")
        }

        async fn find_active_namespace_publishers(
            &self,
            _track_namespace: &str,
        ) -> anyhow::Result<Vec<RelayInfo>> {
            unimplemented!("not used by watched namespace routes")
        }

        async fn find_namespace_publishers_by_prefix(
            &self,
            _track_namespace_prefix: &str,
        ) -> anyhow::Result<Vec<NamespaceRoute>> {
            unimplemented!("not used by watched namespace routes")
        }

        async fn unregister_namespace_publisher(
            &self,
            _track_namespace: &str,
        ) -> anyhow::Result<()> {
            unimplemented!("not used by watched namespace routes")
        }

        async fn unregister_namespace_subscriber(
            &self,
            _track_namespace_prefix: &str,
        ) -> anyhow::Result<()> {
            unimplemented!("not used by watched namespace routes")
        }

        async fn find_namespace_subscribers(
            &self,
            _track_namespace: &str,
        ) -> anyhow::Result<Vec<RelayInfo>> {
            unimplemented!("not used by watched namespace routes")
        }
    }

    fn watched(track_namespaces: &[&str]) -> HashSet<TrackNamespace> {
        track_namespaces.iter().map(ToString::to_string).collect()
    }

    #[tokio::test]
    async fn a_namespace_is_unregistered_after_two_passes_without_a_watcher() {
        // Arrange
        let registry = Arc::new(RecordingRouteRegistry::default());
        let routes = WatchedNamespaceRoutes::new(registry.clone());
        routes.watch("ns").await;

        // Act
        routes.reconcile(&watched(&[])).await;
        let after_first_pass = registry.calls();
        routes.reconcile(&watched(&[])).await;

        // Assert
        assert_eq!(after_first_pass, vec!["register ns"]);
        assert_eq!(registry.calls(), vec!["register ns", "unregister ns"]);
    }

    #[tokio::test]
    async fn a_watcher_seen_between_passes_keeps_the_namespace_registered() {
        // Arrange
        let registry = Arc::new(RecordingRouteRegistry::default());
        let routes = WatchedNamespaceRoutes::new(registry.clone());
        routes.watch("ns").await;

        // Act
        routes.reconcile(&watched(&[])).await;
        routes.reconcile(&watched(&["ns"])).await;
        routes.reconcile(&watched(&[])).await;

        // Assert
        assert_eq!(registry.calls(), vec!["register ns"]);
    }

    #[tokio::test]
    async fn a_watched_namespace_found_by_a_pass_is_registered() {
        // Arrange
        let registry = Arc::new(RecordingRouteRegistry::default());
        let routes = WatchedNamespaceRoutes::new(registry.clone());

        // Act
        routes.reconcile(&watched(&["ns"])).await;

        // Assert
        assert_eq!(registry.calls(), vec!["register ns"]);
    }
}

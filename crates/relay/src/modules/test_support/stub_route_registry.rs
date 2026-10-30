use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;

use crate::modules::{
    cascading::route_registry::{
        NamespaceRoute, RegisterRouteError, RelayInfo, RelayRouteRegistry,
    },
    domain::track_key::TrackKey,
};

pub(crate) enum PublisherLookup {
    NotFound,
    Found {
        namespace_relay_ids: Vec<&'static str>,
        track_relay_ids: Vec<&'static str>,
    },
    Fails,
    MustNotBeCalled,
}

fn relays(relay_ids: &[&str]) -> Vec<RelayInfo> {
    relay_ids
        .iter()
        .map(|relay_id| RelayInfo {
            relay_id: relay_id.to_string(),
            host: relay_id.to_string(),
            port: 4434,
        })
        .collect()
}

pub(crate) struct StubRouteRegistry {
    publisher_lookup: PublisherLookup,
    watched_namespace_calls: Mutex<Vec<String>>,
    migration_target: Option<String>,
    draining: AtomicBool,
    /// The namespaces other relays publish; `None` leaves the namespace routes
    /// unused, so a call fails the test.
    namespace_routes: Option<Vec<String>>,
}

impl StubRouteRegistry {
    pub(crate) fn new(publisher_lookup: PublisherLookup) -> Self {
        Self {
            publisher_lookup,
            watched_namespace_calls: Mutex::new(Vec::new()),
            migration_target: None,
            draining: AtomicBool::new(false),
            namespace_routes: None,
        }
    }

    pub(crate) fn with_namespace_routes(track_namespaces: &[&str]) -> Self {
        Self {
            namespace_routes: Some(
                track_namespaces
                    .iter()
                    .map(|namespace| namespace.to_string())
                    .collect(),
            ),
            ..Self::new(PublisherLookup::MustNotBeCalled)
        }
    }

    pub(crate) fn with_migration_target(migration_target: Option<&str>) -> Self {
        Self {
            migration_target: migration_target.map(str::to_string),
            ..Self::new(PublisherLookup::MustNotBeCalled)
        }
    }

    pub(crate) fn watched_namespace_calls(&self) -> Vec<String> {
        self.watched_namespace_calls.lock().unwrap().clone()
    }

    pub(crate) fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    fn publisher_relay_ids(&self) -> anyhow::Result<(&[&'static str], &[&'static str])> {
        match &self.publisher_lookup {
            PublisherLookup::NotFound => Ok((&[], &[])),
            PublisherLookup::Found {
                namespace_relay_ids,
                track_relay_ids,
            } => Ok((namespace_relay_ids, track_relay_ids)),
            PublisherLookup::Fails => Err(anyhow::anyhow!("route registry down")),
            PublisherLookup::MustNotBeCalled => {
                panic!("a relay requester must not be routed to another relay")
            }
        }
    }
}

#[async_trait]
impl RelayRouteRegistry for StubRouteRegistry {
    async fn find_active_namespace_publishers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        let (namespace_relay_ids, _) = self.publisher_relay_ids()?;
        Ok(relays(namespace_relay_ids))
    }

    async fn find_active_track_publishers(
        &self,
        _track_key: &TrackKey,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        let (_, track_relay_ids) = self.publisher_relay_ids()?;
        Ok(relays(track_relay_ids))
    }

    async fn register_track_publisher(&self, _track_key: &TrackKey) -> anyhow::Result<()> {
        unimplemented!("not used with the stub route registry")
    }

    async fn unregister_track_publisher(&self, _track_key: &TrackKey) -> anyhow::Result<()> {
        unimplemented!("not used with the stub route registry")
    }

    async fn register_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
        self.watched_namespace_calls
            .lock()
            .unwrap()
            .push(format!("register {track_namespace}"));
        Ok(())
    }

    async fn unregister_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
        self.watched_namespace_calls
            .lock()
            .unwrap()
            .push(format!("unregister {track_namespace}"));
        Ok(())
    }

    async fn mark_draining(&self) -> anyhow::Result<()> {
        self.draining.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn find_migration_target(&self) -> anyhow::Result<Option<String>> {
        Ok(self.migration_target.clone())
    }

    async fn register_namespace_publisher(&self, _track_namespace: &str) -> anyhow::Result<()> {
        unimplemented!("not used with the stub route registry")
    }

    async fn register_namespace_subscriber(
        &self,
        _track_namespace_prefix: &str,
    ) -> Result<(), RegisterRouteError> {
        assert!(
            self.namespace_routes.is_some(),
            "not used with the stub route registry"
        );
        Ok(())
    }

    async fn find_namespace_publishers_by_prefix(
        &self,
        _track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<NamespaceRoute>> {
        let routes = self
            .namespace_routes
            .as_ref()
            .expect("not used with the stub route registry");
        Ok(routes
            .iter()
            .map(|track_namespace| NamespaceRoute {
                track_namespace: track_namespace.clone(),
            })
            .collect())
    }

    async fn unregister_namespace_publisher(&self, _track_namespace: &str) -> anyhow::Result<()> {
        unimplemented!("not used with the stub route registry")
    }

    async fn unregister_namespace_subscriber(
        &self,
        _track_namespace_prefix: &str,
    ) -> anyhow::Result<()> {
        unimplemented!("not used with the stub route registry")
    }

    async fn find_namespace_subscribers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        unimplemented!("not used with the stub route registry")
    }
}

use std::sync::Mutex;

use async_trait::async_trait;

use crate::modules::cascading::route_registry::{
    NamespaceRoute, RegisterRouteError, RelayInfo, RelayRouteRegistry,
};

pub(crate) enum PublisherLookup {
    NotFound,
    Fails,
    MustNotBeCalled,
}

pub(crate) struct StubRouteRegistry {
    publisher_lookup: PublisherLookup,
    watched_namespace_calls: Mutex<Vec<String>>,
}

impl StubRouteRegistry {
    pub(crate) fn new(publisher_lookup: PublisherLookup) -> Self {
        Self {
            publisher_lookup,
            watched_namespace_calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn watched_namespace_calls(&self) -> Vec<String> {
        self.watched_namespace_calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl RelayRouteRegistry for StubRouteRegistry {
    async fn find_active_namespace_publishers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        match self.publisher_lookup {
            PublisherLookup::NotFound => Ok(Vec::new()),
            PublisherLookup::Fails => Err(anyhow::anyhow!("route registry down")),
            PublisherLookup::MustNotBeCalled => {
                panic!("a relay requester must not be routed to another relay")
            }
        }
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

    async fn register_namespace_publisher(&self, _track_namespace: &str) -> anyhow::Result<()> {
        unimplemented!("not used with the stub route registry")
    }

    async fn register_namespace_subscriber(
        &self,
        _track_namespace_prefix: &str,
    ) -> Result<(), RegisterRouteError> {
        unimplemented!("not used with the stub route registry")
    }

    async fn find_namespace_publishers_by_prefix(
        &self,
        _track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<NamespaceRoute>> {
        unimplemented!("not used with the stub route registry")
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

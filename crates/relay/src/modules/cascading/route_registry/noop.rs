use async_trait::async_trait;

use super::{NamespaceRoute, RegisterRouteError, RelayInfo, RelayRouteRegistry};

#[derive(Debug)]
pub(crate) struct NoopRelayRouteRegistry;

#[async_trait]
impl RelayRouteRegistry for NoopRelayRouteRegistry {
    async fn register_namespace_publisher(&self, _track_namespace: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn register_namespace_subscriber(
        &self,
        _track_namespace_prefix: &str,
    ) -> Result<(), RegisterRouteError> {
        Ok(())
    }

    async fn find_active_namespace_publishers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        Ok(Vec::new())
    }

    async fn register_watched_namespace(&self, _track_namespace: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn unregister_watched_namespace(&self, _track_namespace: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn find_namespace_publishers_by_prefix(
        &self,
        _track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<NamespaceRoute>> {
        Ok(Vec::new())
    }

    async fn unregister_namespace_publisher(&self, _track_namespace: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn unregister_namespace_subscriber(
        &self,
        _track_namespace_prefix: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn find_namespace_subscribers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        Ok(Vec::new())
    }
}

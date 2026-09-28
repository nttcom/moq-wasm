pub(crate) mod noop;
pub(crate) mod redis;

pub(crate) use noop::NoopRelayRouteRegistry;
pub(crate) use redis::RedisRelayRouteRegistry;

use async_trait::async_trait;

#[derive(Clone, Debug)]
pub(crate) struct RelayInfo {
    pub(crate) relay_id: String,
    pub(crate) host: String,
    pub(crate) port: u16,
}

#[derive(Clone, Debug)]
pub(crate) struct NamespaceRoute {
    pub(crate) track_namespace: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegisterRouteError {
    #[error("namespace route is already registered")]
    Conflict,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[async_trait]
pub(crate) trait RelayRouteRegistry: Send + Sync {
    async fn register_namespace_publisher(
        &self,
        track_namespace: &str,
    ) -> Result<(), RegisterRouteError>;
    async fn register_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> Result<(), RegisterRouteError>;
    async fn find_active_namespace_publisher(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Option<RelayInfo>>;
    async fn find_namespace_publishers_by_prefix(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<NamespaceRoute>>;
    async fn unregister_namespace_publisher(&self, track_namespace: &str) -> anyhow::Result<()>;
    async fn unregister_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<()>;
    async fn find_namespace_subscribers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>>;
}

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
    /// Marks this relay as draining (draft-14 §3.5): other relays stop routing
    /// new subscriptions to it and no longer name it as a migration target.
    async fn mark_draining(&self) -> anyhow::Result<()>;
    /// The client-facing `host[:port]` of the other active relay with the
    /// fewest client sessions, `None` when no other relay is active.
    async fn find_migration_target(&self) -> anyhow::Result<Option<String>>;
    async fn register_namespace_publisher(&self, track_namespace: &str) -> anyhow::Result<()>;
    async fn register_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> Result<(), RegisterRouteError>;
    async fn find_active_namespace_publishers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>>;
    /// A relay watching a namespace (receiving one of its tracks for a client)
    /// is told about every publisher that later announces it, like a
    /// namespace subscriber.
    async fn register_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()>;
    async fn unregister_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()>;
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

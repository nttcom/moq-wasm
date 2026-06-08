pub(crate) mod noop;
pub(crate) mod redis;

pub(crate) use noop::NoopRelayRouteRegistry;
pub(crate) use redis::RedisRelayRouteRegistry;

use async_trait::async_trait;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteStatus {
    Active,
    Draining,
}

impl RouteStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Draining => "draining",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "draining" => Some(Self::Draining),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RelayInfo {
    pub(crate) relay_id: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) status: RouteStatus,
}

#[derive(Clone, Debug)]
pub(crate) struct NamespaceRoute {
    pub(crate) track_namespace: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegisterNamespacePublisherError {
    #[error("namespace already has an active publisher")]
    Conflict,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RegisterNamespaceSubscriberError {
    #[error("namespace already has an active subscriber")]
    Conflict,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[async_trait]
pub(crate) trait RelayRouteRegistry: Send + Sync {
    async fn register_namespace_publisher(
        &self,
        track_namespace: &str,
        status: RouteStatus,
    ) -> Result<(), RegisterNamespacePublisherError>;
    async fn register_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
        status: RouteStatus,
    ) -> Result<(), RegisterNamespaceSubscriberError>;
    async fn find_active_namespace_publisher(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Option<RelayInfo>>;
    async fn find_namespace_publishers_by_prefix(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<NamespaceRoute>>;
    async fn find_namespace_subscribers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>>;
}

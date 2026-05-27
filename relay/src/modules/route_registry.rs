use std::{collections::HashMap, sync::Arc, time::SystemTime};

use async_trait::async_trait;
use redis::AsyncCommands;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RouteStatus {
    Active,
    Draining,
}

impl RouteStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Draining => "draining",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "draining" => Some(Self::Draining),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RelayDescriptor {
    pub(crate) relay_id: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) status: RouteStatus,
}

#[derive(Clone, Debug)]
pub(crate) struct RelayRoute {
    pub(crate) relay: RelayDescriptor,
}

#[derive(Clone, Debug)]
pub(crate) struct RelayNamespaceRoute {
    pub(crate) track_namespace: String,
}

#[async_trait]
pub(crate) trait RelayRouteRegistry: Send + Sync {
    async fn register_relay(&self, relay: &RelayDescriptor) -> anyhow::Result<()>;
    async fn register_namespace_route(
        &self,
        track_namespace: &str,
        status: RouteStatus,
    ) -> anyhow::Result<()>;
    async fn register_track_route(
        &self,
        track_namespace: &str,
        track_name: &str,
        status: RouteStatus,
    ) -> anyhow::Result<()>;
    async fn register_namespace_subscription(
        &self,
        track_namespace_prefix: &str,
        status: RouteStatus,
    ) -> anyhow::Result<()>;
    async fn find_active_track_routes(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> anyhow::Result<Vec<RelayRoute>>;
    async fn find_active_namespace_routes(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayRoute>>;
    async fn find_active_namespace_routes_by_prefix(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<RelayNamespaceRoute>>;
    async fn find_active_namespace_subscribers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayRoute>>;
}

#[derive(Debug)]
pub(crate) struct NoopRelayRouteRegistry;

#[async_trait]
impl RelayRouteRegistry for NoopRelayRouteRegistry {
    async fn register_relay(&self, _relay: &RelayDescriptor) -> anyhow::Result<()> {
        Ok(())
    }

    async fn register_namespace_route(
        &self,
        _track_namespace: &str,
        _status: RouteStatus,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn register_track_route(
        &self,
        _track_namespace: &str,
        _track_name: &str,
        _status: RouteStatus,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn register_namespace_subscription(
        &self,
        _track_namespace_prefix: &str,
        _status: RouteStatus,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn find_active_track_routes(
        &self,
        _track_namespace: &str,
        _track_name: &str,
    ) -> anyhow::Result<Vec<RelayRoute>> {
        Ok(Vec::new())
    }

    async fn find_active_namespace_routes(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayRoute>> {
        Ok(Vec::new())
    }

    async fn find_active_namespace_routes_by_prefix(
        &self,
        _track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<RelayNamespaceRoute>> {
        Ok(Vec::new())
    }

    async fn find_active_namespace_subscribers(
        &self,
        _track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayRoute>> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
pub(crate) struct RedisRelayRouteRegistry {
    relay: RelayDescriptor,
    connection: redis::aio::ConnectionManager,
}

impl RedisRelayRouteRegistry {
    const RELAY_TTL_SECONDS: u64 = 15;
    const ROUTE_TTL_SECONDS: u64 = 60;

    pub(crate) async fn connect(
        redis_url: &str,
        relay: RelayDescriptor,
    ) -> anyhow::Result<Arc<Self>> {
        let client = redis::Client::open(redis_url)?;
        let connection = redis::aio::ConnectionManager::new(client).await?;
        let registry = Arc::new(Self { relay, connection });
        registry.register_relay(&registry.relay).await?;
        registry.spawn_heartbeat();
        Ok(registry)
    }

    fn spawn_heartbeat(self: &Arc<Self>) {
        let registry = self.clone();
        tokio::task::Builder::new()
            .name("Relay Redis Heartbeat")
            .spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if let Err(err) = registry.register_relay(&registry.relay).await {
                        tracing::warn!(?err, relay_id = %registry.relay.relay_id, "failed to refresh relay heartbeat");
                    }
                }
            })
            .expect("failed to spawn relay redis heartbeat");
    }

    fn now_millis() -> u128 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    }

    fn relay_key(relay_id: &str) -> String {
        format!("relay:{relay_id}")
    }

    fn namespace_route_key(track_namespace: &str) -> String {
        format!("route:namespace:{track_namespace}")
    }

    fn namespace_route_prefix() -> &'static str {
        "route:namespace:"
    }

    fn namespace_subscription_key(track_namespace_prefix: &str) -> String {
        format!("route:namespace_subscription:{track_namespace_prefix}")
    }

    fn namespace_subscription_prefix() -> &'static str {
        "route:namespace_subscription:"
    }

    fn track_route_key(track_namespace: &str, track_name: &str) -> String {
        format!("route:track:{track_namespace}:{track_name}")
    }

    async fn relay_descriptor(&self, relay_id: &str) -> anyhow::Result<Option<RelayDescriptor>> {
        let mut connection = self.connection.clone();
        let values: HashMap<String, String> = connection.hgetall(Self::relay_key(relay_id)).await?;
        if values.is_empty() {
            return Ok(None);
        }

        let Some(host) = values.get("host").cloned() else {
            return Ok(None);
        };
        let Some(port) = values
            .get("port")
            .and_then(|value| value.parse::<u16>().ok())
        else {
            return Ok(None);
        };
        let status = values
            .get("status")
            .and_then(|value| RouteStatus::from_str(value))
            .unwrap_or(RouteStatus::Draining);

        Ok(Some(RelayDescriptor {
            relay_id: relay_id.to_string(),
            host,
            port,
            status,
        }))
    }
}

#[async_trait]
impl RelayRouteRegistry for RedisRelayRouteRegistry {
    async fn register_relay(&self, relay: &RelayDescriptor) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let key = Self::relay_key(&relay.relay_id);
        let _: () = connection
            .hset_multiple(
                &key,
                &[
                    ("relay_id", relay.relay_id.as_str()),
                    ("host", relay.host.as_str()),
                    ("port", &relay.port.to_string()),
                    ("status", relay.status.as_str()),
                    ("updated_at", &Self::now_millis().to_string()),
                ],
            )
            .await?;
        let _: () = connection
            .expire(key, Self::RELAY_TTL_SECONDS as i64)
            .await?;
        Ok(())
    }

    async fn register_namespace_route(
        &self,
        track_namespace: &str,
        status: RouteStatus,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let key = Self::namespace_route_key(track_namespace);
        let _: () = connection
            .hset(&key, &self.relay.relay_id, status.as_str())
            .await?;
        let _: () = connection
            .expire(key, Self::ROUTE_TTL_SECONDS as i64)
            .await?;
        Ok(())
    }

    async fn register_track_route(
        &self,
        track_namespace: &str,
        track_name: &str,
        status: RouteStatus,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let key = Self::track_route_key(track_namespace, track_name);
        let _: () = connection
            .hset(&key, &self.relay.relay_id, status.as_str())
            .await?;
        let _: () = connection
            .expire(key, Self::ROUTE_TTL_SECONDS as i64)
            .await?;
        Ok(())
    }

    async fn register_namespace_subscription(
        &self,
        track_namespace_prefix: &str,
        status: RouteStatus,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let key = Self::namespace_subscription_key(track_namespace_prefix);
        let _: () = connection
            .hset(&key, &self.relay.relay_id, status.as_str())
            .await?;
        let _: () = connection
            .expire(key, Self::ROUTE_TTL_SECONDS as i64)
            .await?;
        Ok(())
    }

    async fn find_active_track_routes(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> anyhow::Result<Vec<RelayRoute>> {
        self.find_active_routes(Self::track_route_key(track_namespace, track_name))
            .await
    }

    async fn find_active_namespace_routes(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayRoute>> {
        self.find_active_routes(Self::namespace_route_key(track_namespace))
            .await
    }

    async fn find_active_namespace_routes_by_prefix(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<RelayNamespaceRoute>> {
        let mut connection = self.connection.clone();
        let pattern = format!(
            "{}{}*",
            Self::namespace_route_prefix(),
            track_namespace_prefix
        );
        let keys: Vec<String> = connection.keys(pattern).await?;
        let mut routes = Vec::new();

        for key in keys {
            let Some(track_namespace) = key
                .strip_prefix(Self::namespace_route_prefix())
                .map(ToString::to_string)
            else {
                continue;
            };
            if self.find_active_routes(key).await?.is_empty() {
                continue;
            }
            routes.push(RelayNamespaceRoute { track_namespace });
        }

        Ok(routes)
    }

    async fn find_active_namespace_subscribers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayRoute>> {
        let mut connection = self.connection.clone();
        let pattern = format!("{}*", Self::namespace_subscription_prefix());
        let keys: Vec<String> = connection.keys(pattern).await?;
        let mut routes = Vec::new();

        for key in keys {
            let Some(track_namespace_prefix) =
                key.strip_prefix(Self::namespace_subscription_prefix())
            else {
                continue;
            };
            if !track_namespace.starts_with(track_namespace_prefix) {
                continue;
            }
            routes.extend(self.find_active_routes(key).await?);
        }

        Ok(routes)
    }
}

impl RedisRelayRouteRegistry {
    async fn find_active_routes(&self, key: String) -> anyhow::Result<Vec<RelayRoute>> {
        let mut connection = self.connection.clone();
        let route_statuses: HashMap<String, String> = connection.hgetall(&key).await?;
        let mut routes = Vec::new();

        for (relay_id, status) in route_statuses {
            if relay_id == self.relay.relay_id {
                continue;
            }
            let Some(status) = RouteStatus::from_str(&status) else {
                continue;
            };
            if status != RouteStatus::Active {
                continue;
            }
            let Some(relay) = self.relay_descriptor(&relay_id).await? else {
                let _: () = connection.hdel(&key, relay_id).await?;
                continue;
            };
            if relay.status == RouteStatus::Active {
                routes.push(RelayRoute { relay });
            }
        }

        Ok(routes)
    }
}

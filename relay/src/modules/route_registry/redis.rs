use std::{collections::HashMap, sync::Arc, time::SystemTime};

use async_trait::async_trait;
use redis::AsyncCommands;

use super::{
    NamespaceRoute, RegisterNamespacePublisherError, RegisterNamespaceSubscriberError, RelayInfo,
    RelayRouteRegistry, RouteStatus,
};

#[derive(Clone)]
pub(crate) struct RedisRelayRouteRegistry {
    relay: RelayInfo,
    connection: redis::aio::ConnectionManager,
}

impl RedisRelayRouteRegistry {
    const RELAY_TTL_SECONDS: u64 = 15;
    const ROUTE_TTL_SECONDS: u64 = 15;

    // --- lifecycle ---

    pub(crate) async fn connect(redis_url: &str, relay: RelayInfo) -> anyhow::Result<Arc<Self>> {
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
                    if let Err(err) = registry.refresh_relay_info(&registry.relay.relay_id).await {
                        tracing::warn!(?err, relay_id = %registry.relay.relay_id, "failed to refresh relay heartbeat");
                    }
                    if let Err(err) = registry.refresh_route_ttls().await {
                        tracing::warn!(?err, relay_id = %registry.relay.relay_id, "failed to refresh route ttls");
                    }
                }
            })
            .expect("failed to spawn relay redis heartbeat");
    }

    // --- relay operations ---

    async fn register_relay(&self, relay: &RelayInfo) -> anyhow::Result<()> {
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

    async fn refresh_relay_info(&self, relay_id: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let key = Self::relay_key(relay_id);
        let _: () = connection
            .expire(key, Self::RELAY_TTL_SECONDS as i64)
            .await?;
        Ok(())
    }

    async fn find_relay_info(&self, relay_id: &str) -> anyhow::Result<Option<RelayInfo>> {
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

        Ok(Some(RelayInfo {
            relay_id: relay_id.to_string(),
            host,
            port,
            status,
        }))
    }

    // --- route operations ---

    async fn refresh_route_ttls(&self) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let relay_routes_key = Self::relay_routes_key(&self.relay.relay_id);
        let keys: Vec<String> = connection.smembers(&relay_routes_key).await?;
        if keys.is_empty() {
            return Ok(());
        }
        let mut pipe = redis::pipe();
        for key in &keys {
            pipe.expire(key, Self::ROUTE_TTL_SECONDS as i64).ignore();
        }
        pipe.expire(&relay_routes_key, Self::RELAY_TTL_SECONDS as i64)
            .ignore();
        let _: () = pipe.query_async(&mut connection).await?;
        Ok(())
    }

    async fn find_active_routes(&self, key: String) -> anyhow::Result<Vec<RelayInfo>> {
        let mut connection = self.connection.clone();
        let route_statuses: HashMap<String, String> = connection.hgetall(&key).await?;

        let candidates: Vec<String> = route_statuses
            .into_iter()
            .filter(|(relay_id, status)| {
                relay_id != &self.relay.relay_id
                    && RouteStatus::from_str(status) == Some(RouteStatus::Active)
            })
            .map(|(relay_id, _)| relay_id)
            .collect();

        let mut routes = Vec::new();
        for relay_id in candidates {
            let Some(relay) = self.find_relay_info(&relay_id).await? else {
                let _: () = connection.hdel(&key, &relay_id).await?;
                continue;
            };
            if relay.status == RouteStatus::Active {
                routes.push(relay);
            }
        }
        Ok(routes)
    }

    // --- key builders ---

    fn relay_key(relay_id: &str) -> String {
        format!("relay:{relay_id}")
    }

    fn relay_routes_key(relay_id: &str) -> String {
        format!("relay_routes:{relay_id}")
    }

    fn publisher_namespace_key(track_namespace: &str) -> String {
        format!("route:publisher:namespace:{track_namespace}")
    }

    fn publisher_namespace_prefix() -> &'static str {
        "route:publisher:namespace:"
    }

    fn subscriber_namespace_key(track_namespace_prefix: &str) -> String {
        format!("route:subscriber:namespace:{track_namespace_prefix}")
    }

    // --- utilities ---

    fn now_millis() -> u128 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    }
}

#[async_trait]
impl RelayRouteRegistry for RedisRelayRouteRegistry {
    async fn register_namespace_publisher(
        &self,
        track_namespace: &str,
        status: RouteStatus,
    ) -> Result<(), RegisterNamespacePublisherError> {
        // When registering as Active, atomically check that no other relay is already Active.
        // Draining status (graceful shutdown) skips the conflict check.
        let script = redis::Script::new(include_str!("scripts/register_namespace_publisher.lua"));
        let mut connection = self.connection.clone();
        let key = Self::publisher_namespace_key(track_namespace);
        let relay_routes_key = Self::relay_routes_key(&self.relay.relay_id);
        let result: i64 = script
            .key(&key)
            .key(&relay_routes_key)
            .arg(&self.relay.relay_id)
            .arg(status.as_str())
            .arg(Self::ROUTE_TTL_SECONDS)
            .arg(Self::RELAY_TTL_SECONDS)
            .invoke_async(&mut connection)
            .await
            .map_err(|e| RegisterNamespacePublisherError::Other(e.into()))?;
        if result == 0 {
            return Err(RegisterNamespacePublisherError::Conflict);
        }
        Ok(())
    }

    async fn register_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
        status: RouteStatus,
    ) -> Result<(), RegisterNamespaceSubscriberError> {
        let script = redis::Script::new(include_str!("scripts/register_namespace_subscriber.lua"));
        let mut connection = self.connection.clone();
        let key = Self::subscriber_namespace_key(track_namespace_prefix);
        let relay_routes_key = Self::relay_routes_key(&self.relay.relay_id);
        let result: i64 = script
            .key(&key)
            .key(&relay_routes_key)
            .arg(&self.relay.relay_id)
            .arg(status.as_str())
            .arg(Self::ROUTE_TTL_SECONDS)
            .arg(Self::RELAY_TTL_SECONDS)
            .invoke_async(&mut connection)
            .await
            .map_err(|e| RegisterNamespaceSubscriberError::Other(e.into()))?;
        if result == 0 {
            return Err(RegisterNamespaceSubscriberError::Conflict);
        }
        Ok(())
    }

    async fn find_active_namespace_publisher(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Option<RelayInfo>> {
        Ok(self
            .find_active_routes(Self::publisher_namespace_key(track_namespace))
            .await?
            .into_iter()
            .next())
    }

    async fn find_namespace_publishers_by_prefix(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<NamespaceRoute>> {
        let mut connection = self.connection.clone();
        let pattern = format!(
            "{}{}*",
            Self::publisher_namespace_prefix(),
            track_namespace_prefix
        );
        let keys: Vec<String> = connection.keys(pattern).await?;
        let mut routes = Vec::new();

        for key in keys {
            let Some(track_namespace) = key
                .strip_prefix(Self::publisher_namespace_prefix())
                .map(ToString::to_string)
            else {
                continue;
            };
            if self.find_active_routes(key).await?.is_empty() {
                continue;
            }
            routes.push(NamespaceRoute { track_namespace });
        }

        Ok(routes)
    }

    async fn unregister_namespace_publisher(&self, track_namespace: &str) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let key = Self::publisher_namespace_key(track_namespace);
        let relay_routes_key = Self::relay_routes_key(&self.relay.relay_id);
        let _: () = connection.hdel(&key, &self.relay.relay_id).await?;
        let _: () = connection.srem(relay_routes_key, &key).await?;
        Ok(())
    }

    async fn unregister_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<()> {
        let mut connection = self.connection.clone();
        let key = Self::subscriber_namespace_key(track_namespace_prefix);
        let relay_routes_key = Self::relay_routes_key(&self.relay.relay_id);
        let _: () = connection.hdel(&key, &self.relay.relay_id).await?;
        let _: () = connection.srem(relay_routes_key, &key).await?;
        Ok(())
    }

    async fn find_namespace_subscribers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        let parts: Vec<&str> = track_namespace.split('/').collect();
        let keys: Vec<String> = (1..=parts.len())
            .map(|i| Self::subscriber_namespace_key(&parts[..i].join("/")))
            .collect();

        let script = redis::Script::new(include_str!("scripts/find_namespace_subscribers.lua"));
        let mut invocation = script.prepare_invoke();
        invocation.arg(&self.relay.relay_id);
        for key in &keys {
            invocation.key(key);
        }
        let raw: Vec<String> = invocation
            .invoke_async(&mut self.connection.clone())
            .await?;

        let mut routes = Vec::new();
        for chunk in raw.chunks(4) {
            let [relay_id, host, port_str, status_str] = chunk else {
                continue;
            };
            let port = port_str
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("invalid port in relay info: {port_str}"))?;
            let status = RouteStatus::from_str(status_str)
                .ok_or_else(|| anyhow::anyhow!("unknown route status: {status_str}"))?;
            routes.push(RelayInfo {
                relay_id: relay_id.clone(),
                host: host.clone(),
                port,
                status,
            });
        }

        Ok(routes)
    }
}

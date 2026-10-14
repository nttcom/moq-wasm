use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use async_trait::async_trait;
use redis::AsyncCommands;
use tokio::sync::watch;

use super::{NamespaceRoute, RegisterRouteError, RelayInfo, RelayRouteRegistry};

#[derive(Clone, Copy)]
enum RouteKind {
    Upsert,
    NamespaceSubscriber,
}

impl RouteKind {
    fn register_script(self) -> redis::Script {
        match self {
            Self::Upsert => redis::Script::new(include_str!("scripts/upsert_route.lua")),
            Self::NamespaceSubscriber => {
                redis::Script::new(include_str!("scripts/register_namespace_subscriber.lua"))
            }
        }
    }
}

pub(crate) struct RedisRelayRouteRegistry {
    relay: RelayInfo,
    /// The `host[:port]` clients connect to, named in GOAWAY by a draining
    /// relay; distinct from `relay.host:relay.port`, which other relays dial.
    public_host: String,
    draining: AtomicBool,
    /// Published with the heartbeat so that other relays can pick the least
    /// loaded migration target.
    client_sessions: watch::Receiver<usize>,
    connection: redis::aio::ConnectionManager,
    owned_routes: tokio::sync::Mutex<HashMap<String, RouteKind>>,
}

impl RedisRelayRouteRegistry {
    const RELAY_TTL_SECONDS: u64 = 15;
    const ROUTE_TTL_SECONDS: u64 = 15;
    const ACTIVE_STATUS: &str = "active";
    const DRAINING_STATUS: &str = "draining";
    const PUBLISHER_NAMESPACE_KEY_PREFIX: &str = "route:publisher:namespace:";
    const RELAY_KEY_PREFIX: &str = "relay:";

    pub(crate) async fn connect(
        redis_url: &str,
        relay: RelayInfo,
        public_host: String,
        client_sessions: watch::Receiver<usize>,
    ) -> anyhow::Result<Arc<Self>> {
        let client = redis::Client::open(redis_url)?;
        let connection = redis::aio::ConnectionManager::new(client).await?;
        let registry = Arc::new(Self {
            relay,
            public_host,
            draining: AtomicBool::new(false),
            client_sessions,
            connection,
            owned_routes: tokio::sync::Mutex::new(HashMap::new()),
        });
        registry.register_relay().await?;
        registry.spawn_heartbeat();
        Ok(registry)
    }

    /// Written to the `status` field of the relay hash; `find_active_routes`
    /// and the Lua scripts route only to `active` relays.
    fn status(&self) -> &'static str {
        if self.draining.load(Ordering::SeqCst) {
            Self::DRAINING_STATUS
        } else {
            Self::ACTIVE_STATUS
        }
    }

    fn spawn_heartbeat(self: &Arc<Self>) {
        let registry = self.clone();
        tokio::task::Builder::new()
            .name("Relay Redis Heartbeat")
            .spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    interval.tick().await;
                    if let Err(err) = registry.register_relay().await {
                        tracing::warn!(?err, relay_id = %registry.relay.relay_id, "failed to refresh relay heartbeat");
                    }
                    if let Err(err) = registry.refresh_owned_routes().await {
                        tracing::warn!(?err, relay_id = %registry.relay.relay_id, "failed to refresh owned routes");
                    }
                }
            })
            .expect("failed to spawn relay redis heartbeat");
    }

    async fn register_relay(&self) -> anyhow::Result<()> {
        let relay = &self.relay;
        let mut connection = self.connection.clone();
        let key = Self::relay_key(&relay.relay_id);
        let client_sessions = self.client_sessions.borrow().to_string();
        let _: () = connection
            .hset_multiple(
                &key,
                &[
                    ("relay_id", relay.relay_id.as_str()),
                    ("host", relay.host.as_str()),
                    ("port", &relay.port.to_string()),
                    ("public_host", self.public_host.as_str()),
                    ("client_sessions", &client_sessions),
                    ("status", self.status()),
                    ("updated_at", &Self::now_millis().to_string()),
                ],
            )
            .await?;
        let _: () = connection
            .expire(key, Self::RELAY_TTL_SECONDS as i64)
            .await?;
        Ok(())
    }

    fn parse_relay_info(relay_id: &str, values: &HashMap<String, String>) -> Option<RelayInfo> {
        let host = values.get("host")?.clone();
        let port = values.get("port")?.parse::<u16>().ok()?;
        Some(RelayInfo {
            relay_id: relay_id.to_string(),
            host,
            port,
        })
    }

    async fn refresh_owned_routes(&self) -> anyhow::Result<()> {
        let owned_routes = self.owned_routes.lock().await;
        if owned_routes.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection.clone();
        let mut pipe = redis::pipe();
        pipe.atomic();
        for key in owned_routes.keys() {
            pipe.hexists(key, &self.relay.relay_id)
                .expire(key, Self::ROUTE_TTL_SECONDS as i64)
                .ignore();
        }
        let still_registered: Vec<bool> = pipe.query_async(&mut connection).await?;

        for ((key, kind), still_registered) in owned_routes.iter().zip(still_registered) {
            if still_registered {
                continue;
            }
            match self.invoke_register_script(*kind, key).await {
                Ok(()) => tracing::warn!(route_key = %key, "restored route lost from redis"),
                Err(err) => {
                    tracing::warn!(?err, route_key = %key, "failed to restore route lost from redis")
                }
            }
        }
        Ok(())
    }

    async fn find_active_routes(&self, key: String) -> anyhow::Result<Vec<RelayInfo>> {
        let mut connection = self.connection.clone();
        let route_statuses: HashMap<String, String> = connection.hgetall(&key).await?;

        let candidates: Vec<String> = route_statuses
            .into_iter()
            .filter(|(relay_id, status)| {
                relay_id != &self.relay.relay_id && status == Self::ACTIVE_STATUS
            })
            .map(|(relay_id, _)| relay_id)
            .collect();

        let mut routes = Vec::new();
        for relay_id in candidates {
            let values: HashMap<String, String> =
                connection.hgetall(Self::relay_key(&relay_id)).await?;
            let Some(relay) = Self::parse_relay_info(&relay_id, &values) else {
                let _: () = connection.hdel(&key, &relay_id).await?;
                continue;
            };
            if values.get("status").map(String::as_str) == Some(Self::ACTIVE_STATUS) {
                routes.push(relay);
            }
        }
        Ok(routes)
    }

    async fn register_route(&self, kind: RouteKind, key: String) -> Result<(), RegisterRouteError> {
        let mut owned_routes = self.owned_routes.lock().await;
        self.invoke_register_script(kind, &key).await?;
        owned_routes.insert(key, kind);
        Ok(())
    }

    async fn invoke_register_script(
        &self,
        kind: RouteKind,
        key: &str,
    ) -> Result<(), RegisterRouteError> {
        let mut connection = self.connection.clone();
        let registered: i64 = kind
            .register_script()
            .key(key)
            .arg(&self.relay.relay_id)
            .arg(Self::ROUTE_TTL_SECONDS)
            .invoke_async(&mut connection)
            .await
            .map_err(|e| RegisterRouteError::Other(e.into()))?;
        if registered == 0 {
            return Err(RegisterRouteError::Conflict);
        }
        Ok(())
    }

    async fn unregister_route(&self, key: &str) -> anyhow::Result<()> {
        let mut owned_routes = self.owned_routes.lock().await;
        owned_routes.remove(key);
        let mut connection = self.connection.clone();
        let _: () = connection.hdel(key, &self.relay.relay_id).await?;
        Ok(())
    }

    fn relay_key(relay_id: &str) -> String {
        format!("{}{relay_id}", Self::RELAY_KEY_PREFIX)
    }

    /// The `active` relay other than `self` with the fewest client sessions,
    /// the lowest relay id among equals; a relay that reports no count ranks
    /// last.
    fn pick_migration_target(
        self_relay_id: &str,
        relays: &[(String, HashMap<String, String>)],
    ) -> Option<String> {
        relays
            .iter()
            .filter(|(relay_id, values)| {
                relay_id != self_relay_id
                    && values.get("status").map(String::as_str) == Some(Self::ACTIVE_STATUS)
            })
            .filter_map(|(relay_id, values)| {
                let public_host = values.get("public_host")?;
                let client_sessions = values
                    .get("client_sessions")
                    .and_then(|count| count.parse::<usize>().ok())
                    .unwrap_or(usize::MAX);
                (!public_host.is_empty()).then(|| (client_sessions, relay_id, public_host.clone()))
            })
            .min_by(|(load_a, id_a, _), (load_b, id_b, _)| (load_a, id_a).cmp(&(load_b, id_b)))
            .map(|(_, _, public_host)| public_host)
    }

    fn publisher_namespace_key(track_namespace: &str) -> String {
        format!("{}{track_namespace}", Self::PUBLISHER_NAMESPACE_KEY_PREFIX)
    }

    fn subscriber_namespace_key(track_namespace_prefix: &str) -> String {
        format!("route:subscriber:namespace:{track_namespace_prefix}")
    }

    fn watched_namespace_key(track_namespace: &str) -> String {
        format!("route:subscriber:watched:{track_namespace}")
    }

    fn now_millis() -> u128 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default()
    }
}

#[async_trait]
impl RelayRouteRegistry for RedisRelayRouteRegistry {
    async fn mark_draining(&self) -> anyhow::Result<()> {
        self.draining.store(true, Ordering::SeqCst);
        self.register_relay().await
    }

    async fn find_migration_target(&self) -> anyhow::Result<Option<String>> {
        let mut connection = self.connection.clone();
        let keys: Vec<String> = connection
            .keys(format!("{}*", Self::RELAY_KEY_PREFIX))
            .await?;
        let mut relays = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(relay_id) = key.strip_prefix(Self::RELAY_KEY_PREFIX) else {
                continue;
            };
            let values: HashMap<String, String> = connection.hgetall(&key).await?;
            relays.push((relay_id.to_string(), values));
        }
        Ok(Self::pick_migration_target(&self.relay.relay_id, &relays))
    }

    async fn register_namespace_publisher(&self, track_namespace: &str) -> anyhow::Result<()> {
        Ok(self
            .register_route(
                RouteKind::Upsert,
                Self::publisher_namespace_key(track_namespace),
            )
            .await?)
    }

    async fn register_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> Result<(), RegisterRouteError> {
        self.register_route(
            RouteKind::NamespaceSubscriber,
            Self::subscriber_namespace_key(track_namespace_prefix),
        )
        .await
    }

    async fn find_active_namespace_publishers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        self.find_active_routes(Self::publisher_namespace_key(track_namespace))
            .await
    }

    async fn register_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
        Ok(self
            .register_route(
                RouteKind::Upsert,
                Self::watched_namespace_key(track_namespace),
            )
            .await?)
    }

    async fn unregister_watched_namespace(&self, track_namespace: &str) -> anyhow::Result<()> {
        self.unregister_route(&Self::watched_namespace_key(track_namespace))
            .await
    }

    async fn find_namespace_publishers_by_prefix(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<Vec<NamespaceRoute>> {
        let mut connection = self.connection.clone();
        let pattern = format!(
            "{}{}*",
            Self::PUBLISHER_NAMESPACE_KEY_PREFIX,
            track_namespace_prefix
        );
        let keys: Vec<String> = connection.keys(pattern).await?;
        let mut routes = Vec::new();

        for key in keys {
            let Some(track_namespace) = key
                .strip_prefix(Self::PUBLISHER_NAMESPACE_KEY_PREFIX)
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
        self.unregister_route(&Self::publisher_namespace_key(track_namespace))
            .await
    }

    async fn unregister_namespace_subscriber(
        &self,
        track_namespace_prefix: &str,
    ) -> anyhow::Result<()> {
        self.unregister_route(&Self::subscriber_namespace_key(track_namespace_prefix))
            .await
    }

    async fn find_namespace_subscribers(
        &self,
        track_namespace: &str,
    ) -> anyhow::Result<Vec<RelayInfo>> {
        let parts: Vec<&str> = track_namespace.split('/').collect();
        let keys: Vec<String> = (1..=parts.len())
            .map(|i| Self::subscriber_namespace_key(&parts[..i].join("/")))
            .chain([Self::watched_namespace_key(track_namespace)])
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
            let [relay_id, host, port_str, _active_status] = chunk else {
                continue;
            };
            let port = port_str
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("invalid port in relay info: {port_str}"))?;
            routes.push(RelayInfo {
                relay_id: relay_id.clone(),
                host: host.clone(),
                port,
            });
        }

        Ok(routes)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::RedisRelayRouteRegistry;

    fn relay(status: &str, public_host: &str) -> HashMap<String, String> {
        HashMap::from([
            ("status".to_string(), status.to_string()),
            ("public_host".to_string(), public_host.to_string()),
        ])
    }

    fn loaded_relay(public_host: &str, client_sessions: usize) -> HashMap<String, String> {
        let mut values = relay("active", public_host);
        values.insert("client_sessions".to_string(), client_sessions.to_string());
        values
    }

    #[test]
    fn migration_target_is_the_least_loaded_active_relay() {
        // Arrange
        let relays = [
            ("relay-a".to_string(), loaded_relay("a.example:4433", 12)),
            ("relay-b".to_string(), loaded_relay("b.example:4433", 3)),
            ("relay-c".to_string(), relay("active", "c.example:4433")),
        ];

        // Act
        let target = RedisRelayRouteRegistry::pick_migration_target("relay-self", &relays);

        // Assert: relay-c reports no count and ranks last
        assert_eq!(target.as_deref(), Some("b.example:4433"));
    }

    #[test]
    fn migration_target_is_the_lowest_active_relay_other_than_self() {
        // Arrange
        let relays = [
            ("relay-c".to_string(), relay("active", "c.example:4433")),
            ("relay-a".to_string(), relay("active", "a.example:4433")),
            ("relay-b".to_string(), relay("active", "b.example:4433")),
        ];

        // Act
        let target = RedisRelayRouteRegistry::pick_migration_target("relay-a", &relays);

        // Assert
        assert_eq!(target.as_deref(), Some("b.example:4433"));
    }

    #[test]
    fn draining_relays_and_relays_without_a_public_host_are_not_targets() {
        // Arrange
        let relays = [
            ("relay-a".to_string(), relay("draining", "a.example:4433")),
            ("relay-b".to_string(), relay("active", "")),
            ("relay-c".to_string(), relay("active", "c.example:4433")),
        ];

        // Act
        let target = RedisRelayRouteRegistry::pick_migration_target("relay-self", &relays);

        // Assert
        assert_eq!(target.as_deref(), Some("c.example:4433"));
    }

    #[test]
    fn no_target_when_this_relay_is_the_only_active_one() {
        // Arrange
        let relays = [("relay-a".to_string(), relay("active", "a.example:4433"))];

        // Act
        let target = RedisRelayRouteRegistry::pick_migration_target("relay-a", &relays);

        // Assert
        assert_eq!(target, None);
    }
}

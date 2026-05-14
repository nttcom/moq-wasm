use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use tokio::sync::RwLock;

use crate::modules::{
    core::handler::publish::PublishHandler,
    sequences::tables::table::{
        ActiveUpstreamSubscription, RemovedDownstreamSubscription, Table, UpstreamSubscriptionKey,
    },
    types::{SessionId, TrackNamespace, TrackNamespacePrefix},
};

#[derive(Debug)]
pub(crate) struct HashMapTable {
    /**
     * namespace mechanism
     * publish_namespace: room/member
     * subscriber_namespace: room/
     * publish: room/member + video
     * subscribe: room/member/video
     */
    pub(crate) publisher_namespaces: DashMap<TrackNamespace, SessionId>,
    pub(crate) subscriber_namespaces: DashMap<TrackNamespacePrefix, DashSet<SessionId>>,
    pub(crate) published_handlers: RwLock<Vec<(SessionId, Arc<dyn PublishHandler>)>>,
    pub(crate) track_alias_links: DashMap<(SessionId, u64, SessionId), u64>,
    pub(crate) active_upstream_subscriptions:
        DashMap<UpstreamSubscriptionKey, ActiveUpstreamSubscription>,
    pub(crate) downstream_subscriptions: DashMap<(SessionId, u64), UpstreamSubscriptionKey>,
}

#[async_trait::async_trait]
impl Table for HashMapTable {
    fn new() -> Self {
        Self {
            publisher_namespaces: DashMap::new(),
            subscriber_namespaces: DashMap::new(),
            published_handlers: RwLock::new(Vec::new()),
            track_alias_links: DashMap::new(),
            active_upstream_subscriptions: DashMap::new(),
            downstream_subscriptions: DashMap::new(),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.remove_session",
        skip_all,
        fields(session_id = %session_id)
    )]
    async fn remove_session(&self, session_id: SessionId) {
        let namespaces_to_remove: Vec<_> = self
            .publisher_namespaces
            .iter()
            .filter_map(|entry| (*entry.value() == session_id).then(|| entry.key().clone()))
            .collect();
        for track_namespace in namespaces_to_remove {
            self.publisher_namespaces.remove(&track_namespace);
        }

        let empty_prefixes: Vec<_> = self
            .subscriber_namespaces
            .iter()
            .filter_map(|entry| {
                entry.value().remove(&session_id);
                entry.value().is_empty().then(|| entry.key().clone())
            })
            .collect();
        for track_namespace_prefix in empty_prefixes {
            self.subscriber_namespaces.remove(&track_namespace_prefix);
        }

        self.published_handlers
            .write()
            .await
            .retain(|(registered_session_id, _)| *registered_session_id != session_id);

        let aliases_to_remove: Vec<_> = self
            .track_alias_links
            .iter()
            .filter_map(|entry| {
                let (publisher_session_id, publisher_track_alias, subscriber_session_id) =
                    *entry.key();
                (publisher_session_id == session_id || subscriber_session_id == session_id)
                    .then_some({
                        (
                            publisher_session_id,
                            publisher_track_alias,
                            subscriber_session_id,
                        )
                    })
            })
            .collect();
        for key in aliases_to_remove {
            self.track_alias_links.remove(&key);
        }

        let downstream_keys: Vec<_> = self
            .downstream_subscriptions
            .iter()
            .filter_map(|entry| (entry.key().0 == session_id).then_some(*entry.key()))
            .collect();
        for key in downstream_keys {
            let _ = self.remove_downstream_subscription(key.0, key.1);
        }

        let upstream_keys: Vec<_> = self
            .active_upstream_subscriptions
            .iter()
            .filter_map(|entry| {
                (entry.key().publisher_session_id == session_id).then_some(entry.key().clone())
            })
            .collect();
        for key in upstream_keys {
            self.active_upstream_subscriptions.remove(&key);
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.register_publish_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace)
    )]
    fn register_publish_namespace(&self, session_id: SessionId, track_namespace: String) -> bool {
        if self
            .publisher_namespaces
            .get_mut(&track_namespace)
            .is_some()
        {
            tracing::error!(
                "'{}' is registered for namespace publication.",
                track_namespace
            );
            false
        } else {
            self.publisher_namespaces
                .insert(track_namespace.to_string(), session_id);
            true
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.register_subscribe_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    fn register_subscribe_namespace(&self, session_id: SessionId, track_namespace_prefix: String) {
        if let Some(dash_set) = self.subscriber_namespaces.get_mut(&track_namespace_prefix) {
            dash_set.insert(session_id);
        } else {
            tracing::info!(
                session_id = %session_id,
                track_namespace_prefix = %track_namespace_prefix,
                "New namespace prefix is subscribed."
            );
            let dash_set = DashSet::new();
            dash_set.insert(session_id);
            self.subscriber_namespaces
                .insert(track_namespace_prefix.to_string(), dash_set);
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.register_publish",
        skip_all,
        fields(session_id = %session_id)
    )]
    async fn register_publish(&self, session_id: SessionId, handler: Arc<dyn PublishHandler>) {
        self.published_handlers
            .write()
            .await
            .push((session_id, handler));
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.get_namespace_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    fn get_namespace_subscribers(&self, track_namespace: &str) -> DashSet<SessionId> {
        let combined = DashSet::new();
        self.subscriber_namespaces
            .iter()
            // Check if the published namespace (track_namespace) falls under the subscribed prefix (entry.key())
            // Example: Published "room/member" starts with Subscribed "room" -> Match
            .filter(|entry| track_namespace.starts_with(entry.key()))
            .for_each(|entry| {
                entry.value().iter().for_each(|session_id| {
                    combined.insert(*session_id);
                })
            });
        combined
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.get_subscribers",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    async fn get_subscribers(
        &self,
        track_namespace_prefix: &str,
    ) -> DashSet<(String, (Option<String>, Option<u64>))> {
        let filtered = DashSet::new();
        for entry in self.publisher_namespaces.iter() {
            if entry.key().starts_with(track_namespace_prefix) {
                filtered.insert((entry.key().clone(), (None, None)));
            }
        }

        for (_, handler) in self.published_handlers.read().await.iter() {
            if handler
                .track_namespace()
                .starts_with(track_namespace_prefix)
            {
                filtered.insert((
                    handler.track_namespace().to_string(),
                    (
                        Some(handler.track_name().to_string()),
                        Some(handler.track_alias()),
                    ),
                ));
            }
        }
        filtered
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.get_publish_namespace",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    fn get_publish_namespace(&self, track_namespace: &str) -> Option<SessionId> {
        let result = self.publisher_namespaces.get(track_namespace)?;
        Some(*result)
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.find_publish_handler_with",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    async fn find_publish_handler_with(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<(SessionId, Arc<dyn PublishHandler>)> {
        let handlers = self.published_handlers.read().await;
        if let Some((session_id, handler)) = handlers
            .iter()
            .find(|(_, h)| h.track_namespace() == track_namespace && h.track_name() == track_name)
        {
            Some((*session_id, handler.clone()))
        } else {
            None
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.register_track_alias_link",
        skip_all,
        fields(
            publisher_session_id = %publisher_session_id,
            publisher_track_alias = %publisher_track_alias,
            subscriber_session_id = %subscriber_session_id,
            subscriber_track_alias = %subscriber_track_alias
        )
    )]
    fn register_track_alias_link(
        &self,
        publisher_session_id: SessionId,
        publisher_track_alias: u64,
        subscriber_session_id: SessionId,
        subscriber_track_alias: u64,
    ) {
        self.track_alias_links.insert(
            (
                publisher_session_id,
                publisher_track_alias,
                subscriber_session_id,
            ),
            subscriber_track_alias,
        );
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.table.find_subscriber_track_alias",
        skip_all,
        fields(
            publisher_session_id = %publisher_session_id,
            publisher_track_alias = %publisher_track_alias,
            subscriber_session_id = %subscriber_session_id
        )
    )]
    fn _find_subscriber_track_alias(
        &self,
        publisher_session_id: SessionId,
        publisher_track_alias: u64,
        subscriber_session_id: SessionId,
    ) -> Option<u64> {
        self.track_alias_links
            .get(&(
                publisher_session_id,
                publisher_track_alias,
                subscriber_session_id,
            ))
            .map(|value| *value)
    }

    fn get_active_upstream_subscription(
        &self,
        publisher_session_id: SessionId,
        track_namespace: &str,
        track_name: &str,
    ) -> Option<ActiveUpstreamSubscription> {
        self.active_upstream_subscriptions
            .get(&UpstreamSubscriptionKey {
                publisher_session_id,
                track_namespace: track_namespace.to_string(),
                track_name: track_name.to_string(),
            })
            .map(|entry| entry.value().clone())
    }

    fn register_upstream_subscription(
        &self,
        key: UpstreamSubscriptionKey,
        subscription: ActiveUpstreamSubscription,
    ) {
        self.active_upstream_subscriptions.insert(key, subscription);
    }

    fn register_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        upstream_key: UpstreamSubscriptionKey,
    ) -> bool {
        let Some(mut entry) = self.active_upstream_subscriptions.get_mut(&upstream_key) else {
            return false;
        };
        entry.downstream_subscriber_count += 1;
        drop(entry);

        self.downstream_subscriptions.insert(
            (downstream_session_id, downstream_subscribe_id),
            upstream_key,
        );
        true
    }

    fn remove_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<RemovedDownstreamSubscription> {
        let (_, upstream_key) = self
            .downstream_subscriptions
            .remove(&(downstream_session_id, downstream_subscribe_id))?;
        let mut entry = self.active_upstream_subscriptions.get_mut(&upstream_key)?;
        if entry.downstream_subscriber_count > 0 {
            entry.downstream_subscriber_count -= 1;
        }
        let removed = RemovedDownstreamSubscription {
            upstream_key: upstream_key.clone(),
            upstream_subscribe_id: entry.upstream_subscribe_id,
            remaining_downstream_subscriber_count: entry.downstream_subscriber_count,
        };
        let should_remove = entry.downstream_subscriber_count == 0;
        drop(entry);
        if should_remove {
            self.active_upstream_subscriptions.remove(&upstream_key);
        }
        Some(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::enums::{ContentExists, FilterType, GroupOrder};

    #[derive(Debug)]
    struct StubPublishHandler {
        track_namespace: String,
        track_name: String,
        track_alias: u64,
    }

    #[async_trait::async_trait]
    impl PublishHandler for StubPublishHandler {
        fn track_namespace(&self) -> &str {
            &self.track_namespace
        }

        fn track_name(&self) -> &str {
            &self.track_name
        }

        fn track_alias(&self) -> u64 {
            self.track_alias
        }

        fn _group_order(&self) -> GroupOrder {
            GroupOrder::Ascending
        }

        fn _content_exists(&self) -> ContentExists {
            ContentExists::False
        }

        fn _forward(&self) -> bool {
            true
        }

        fn _authorization_token(&self) -> Option<String> {
            None
        }

        fn _delivery_timeout(&self) -> Option<u64> {
            None
        }

        fn _max_cache_duration(&self) -> Option<u64> {
            None
        }

        async fn ok(
            &self,
            _subscriber_priority: u8,
            _filter_type: FilterType,
            _expires: u64,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn _error(&self, _code: u64, _reason_phrase: String) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn remove_session_cleans_up_all_session_scoped_entries() {
        let table = HashMapTable::new();

        assert!(table.register_publish_namespace(1, "room/member".to_string()));
        table.register_subscribe_namespace(1, "room/".to_string());
        table.register_subscribe_namespace(2, "room/".to_string());
        table.register_subscribe_namespace(1, "solo/".to_string());
        table
            .register_publish(
                1,
                Arc::new(StubPublishHandler {
                    track_namespace: "room/member".to_string(),
                    track_name: "video".to_string(),
                    track_alias: 10,
                }),
            )
            .await;
        table.register_track_alias_link(1, 10, 2, 20);
        table.register_track_alias_link(2, 30, 1, 40);

        table.remove_session(1).await;

        assert!(table.get_publish_namespace("room/member").is_none());
        assert!(
            table
                .find_publish_handler_with("room/member", "video")
                .await
                .is_none()
        );
        assert!(table._find_subscriber_track_alias(1, 10, 2).is_none());
        assert!(table._find_subscriber_track_alias(2, 30, 1).is_none());

        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
        assert!(!room_subscribers.contains(&1));

        assert!(table.subscriber_namespaces.get("solo/").is_none());
    }
}

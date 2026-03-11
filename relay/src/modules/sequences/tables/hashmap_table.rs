use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use tokio::sync::RwLock;

use crate::modules::{
    core::handler::publish::PublishHandler,
    sequences::tables::table::Table,
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
}

#[async_trait::async_trait]
impl Table for HashMapTable {
    fn new() -> Self {
        Self {
            publisher_namespaces: DashMap::new(),
            subscriber_namespaces: DashMap::new(),
            published_handlers: RwLock::new(Vec::new()),
            track_alias_links: DashMap::new(),
        }
    }

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
            tracing::info!("New namespace '{}' is subscribed.", track_namespace);
            self.publisher_namespaces
                .insert(track_namespace.to_string(), session_id);
            true
        }
    }

    fn register_subscribe_namespace(&self, session_id: SessionId, track_namespace_prefix: String) {
        if let Some(dash_set) = self.subscriber_namespaces.get_mut(&track_namespace_prefix) {
            tracing::info!(
                "The namespace prefix '{}' is registered for namespace subscription.",
                track_namespace_prefix
            );
            dash_set.insert(session_id);
        } else {
            tracing::info!(
                "New namespace prefix '{}' is subscribed.",
                track_namespace_prefix
            );
            let dash_set = DashSet::new();
            dash_set.insert(session_id);
            self.subscriber_namespaces
                .insert(track_namespace_prefix.to_string(), dash_set);
        }
    }

    async fn register_publish(&self, session_id: SessionId, handler: Arc<dyn PublishHandler>) {
        self.published_handlers
            .write()
            .await
            .push((session_id, handler));
    }

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

    fn get_publish_namespace(&self, track_namespace: &str) -> Option<SessionId> {
        let result = self.publisher_namespaces.get(track_namespace)?;
        Some(*result)
    }

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

    fn find_subscriber_track_alias(
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
}

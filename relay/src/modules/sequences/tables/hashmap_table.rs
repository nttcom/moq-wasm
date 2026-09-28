use std::sync::Arc;

use dashmap::{DashMap, DashSet, Entry};
use tokio::sync::{RwLock, oneshot};

use crate::modules::{
    core::handler::publish::PublishHandler,
    sequences::tables::table::{
        ActiveUpstreamSubscription, DownstreamSubscription, PeerKind,
        RemovedDownstreamSubscription, RemovedSessionSubscriptions, UpstreamSubscriptionKey,
        UpstreamSubscriptionOrigin,
    },
    types::{SessionId, TrackNamespace, TrackNamespacePrefix},
};

#[derive(Debug)]
pub(crate) struct RegisteredDownstreamSubscription {
    pub(crate) subscription: DownstreamSubscription,
    _runner_stop_sender: oneshot::Sender<()>,
}

#[derive(Debug)]
pub(crate) struct InMemoryLocalPubSubDirectory {
    /**
     * namespace mechanism
     * publish_namespace: room/member
     * subscriber_namespace: room/
     * publish: room/member + video
     * subscribe: room/member/video
     */
    pub(crate) publisher_namespaces: DashMap<TrackNamespace, DashMap<SessionId, PeerKind>>,
    pub(crate) subscriber_namespaces: DashMap<TrackNamespacePrefix, DashMap<SessionId, PeerKind>>,
    pub(crate) published_handlers: RwLock<Vec<(SessionId, Arc<dyn PublishHandler>)>>,
    pub(crate) track_alias_links: DashMap<(SessionId, u64, SessionId), u64>,
    pub(crate) active_upstream_subscriptions:
        DashMap<UpstreamSubscriptionKey, ActiveUpstreamSubscription>,
    pub(crate) downstream_subscriptions:
        DashMap<(SessionId, u64), RegisteredDownstreamSubscription>,
}

impl InMemoryLocalPubSubDirectory {
    pub(crate) fn new() -> Self {
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
        name = "relay.local_pub_sub_directory.remove_session",
        skip_all,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn remove_session(
        &self,
        session_id: SessionId,
    ) -> RemovedSessionSubscriptions {
        let mut removed = RemovedSessionSubscriptions::default();

        let mut empty_namespaces = Vec::new();
        for entry in self.publisher_namespaces.iter() {
            let removed_kind = entry.value().remove(&session_id).map(|(_, kind)| kind);
            // Report the namespace for Redis cleanup only when the removed session
            // was the last client publisher; relay publishers don't own routes.
            let no_clients_remain = !entry
                .value()
                .iter()
                .any(|session| *session.value() == PeerKind::Client);
            if removed_kind == Some(PeerKind::Client) && no_clients_remain {
                removed
                    .publish_namespace_track_namespaces
                    .push(entry.key().clone());
            }
            if entry.value().is_empty() {
                empty_namespaces.push(entry.key().clone());
            }
        }
        for track_namespace in empty_namespaces {
            self.publisher_namespaces.remove(&track_namespace);
        }

        let mut empty_prefixes = Vec::new();
        for entry in self.subscriber_namespaces.iter() {
            let removed_kind = entry.value().remove(&session_id).map(|(_, kind)| kind);
            // Report the prefix for Redis cleanup only when the removed session
            // was the last client subscriber; relay subscribers don't own routes.
            let no_clients_remain = !entry
                .value()
                .iter()
                .any(|session| *session.value() == PeerKind::Client);
            if removed_kind == Some(PeerKind::Client) && no_clients_remain {
                removed
                    .subscribe_namespace_prefixes
                    .push(entry.key().clone());
            }
            if entry.value().is_empty() {
                empty_prefixes.push(entry.key().clone());
            }
        }
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
            if let Some(subscription) = self.remove_downstream_subscription(key.0, key.1) {
                removed.downstream_subscriptions.push(subscription);
            }
        }

        let upstream_keys: Vec<_> = self
            .active_upstream_subscriptions
            .iter()
            .filter_map(|entry| {
                (entry.key().publisher_session_id == session_id).then_some(entry.key().clone())
            })
            .collect();
        for upstream_key in upstream_keys {
            let Some((_, active_subscription)) =
                self.active_upstream_subscriptions.remove(&upstream_key)
            else {
                continue;
            };
            removed
                .upstream_track_keys
                .push(active_subscription.track_key.clone());

            let downstream_keys: Vec<_> = self
                .downstream_subscriptions
                .iter()
                .filter_map(|entry| {
                    (entry.value().subscription.upstream_key == upstream_key)
                        .then_some(*entry.key())
                })
                .collect();
            for downstream_key in downstream_keys {
                self.downstream_subscriptions.remove(&downstream_key);
                removed
                    .downstream_subscriptions
                    .push(RemovedDownstreamSubscription {
                        upstream_key: upstream_key.clone(),
                        upstream_request_id: active_subscription.upstream_request_id,
                        track_key: active_subscription.track_key.clone(),
                        remaining_downstream_subscriber_count: 0,
                        upstream_origin: active_subscription.origin,
                    });
            }
        }

        removed
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.register_publish_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace, peer_kind = ?peer_kind)
    )]
    pub(crate) fn register_publish_namespace(
        &self,
        session_id: SessionId,
        track_namespace: String,
        peer_kind: PeerKind,
    ) -> bool {
        if let Some(sessions) = self.publisher_namespaces.get_mut(&track_namespace) {
            sessions.insert(session_id, peer_kind);
        } else {
            let sessions = DashMap::new();
            sessions.insert(session_id, peer_kind);
            self.publisher_namespaces.insert(track_namespace, sessions);
        }
        true
    }

    /// Returns true when no client publisher remains for the namespace,
    /// i.e. the caller may clean up the Redis route.
    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.unregister_publish_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace)
    )]
    pub(crate) fn unregister_publish_namespace(
        &self,
        session_id: SessionId,
        track_namespace: &str,
    ) -> bool {
        let Some(sessions) = self.publisher_namespaces.get(track_namespace) else {
            return true;
        };

        sessions.remove(&session_id);
        let no_clients_remain = !sessions
            .iter()
            .any(|session| *session.value() == PeerKind::Client);
        let is_empty = sessions.is_empty();
        drop(sessions);

        if is_empty {
            self.publisher_namespaces.remove(track_namespace);
        }

        no_clients_remain
    }

    /// Drops relay-origin publisher namespaces under the prefix once no
    /// client subscriber covers them anymore. Relay-learned namespaces are
    /// only withdrawn by a best-effort PUBLISH_NAMESPACE_DONE, which can be
    /// missed when the remote publisher and the local subscriber leave at the
    /// same time; purging here lets the next subscriber re-learn them from
    /// the route registry instead of a stale local copy.
    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.purge_relay_publish_namespaces",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) fn purge_relay_publish_namespaces(&self, track_namespace_prefix: &str) {
        let mut empty_namespaces = Vec::new();
        for entry in self.publisher_namespaces.iter() {
            if !entry.key().starts_with(track_namespace_prefix) {
                continue;
            }
            // Keep namespaces that another client-subscribed prefix still covers.
            let covered = self.subscriber_namespaces.iter().any(|prefix_entry| {
                entry.key().starts_with(prefix_entry.key())
                    && prefix_entry
                        .value()
                        .iter()
                        .any(|session| *session.value() == PeerKind::Client)
            });
            if covered {
                continue;
            }

            let relay_session_ids: Vec<SessionId> = entry
                .value()
                .iter()
                .filter(|session| *session.value() == PeerKind::Relay)
                .map(|session| *session.key())
                .collect();
            for relay_session_id in relay_session_ids {
                tracing::info!(
                    session_id = %relay_session_id,
                    track_namespace = %entry.key(),
                    "purged relay-origin publish namespace"
                );
                entry.value().remove(&relay_session_id);
            }
            if entry.value().is_empty() {
                empty_namespaces.push(entry.key().clone());
            }
        }
        for track_namespace in empty_namespaces {
            self.publisher_namespaces.remove(&track_namespace);
        }
    }

    /// Returns true when this registration adds the first client subscriber
    /// for the prefix, i.e. the caller should register the Redis route.
    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.register_subscribe_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix, peer_kind = ?peer_kind)
    )]
    pub(crate) fn register_subscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: String,
        peer_kind: PeerKind,
    ) -> bool {
        if let Some(sessions) = self.subscriber_namespaces.get_mut(&track_namespace_prefix) {
            let had_client = sessions
                .iter()
                .any(|session| *session.value() == PeerKind::Client);
            sessions.insert(session_id, peer_kind);
            peer_kind == PeerKind::Client && !had_client
        } else {
            tracing::info!(
                session_id = %session_id,
                track_namespace_prefix = %track_namespace_prefix,
                "New namespace prefix is subscribed."
            );
            let sessions = DashMap::new();
            sessions.insert(session_id, peer_kind);
            self.subscriber_namespaces
                .insert(track_namespace_prefix, sessions);
            peer_kind == PeerKind::Client
        }
    }

    /// Returns true when no client subscriber remains for the prefix,
    /// i.e. the caller may clean up the Redis route.
    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.unregister_subscribe_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) fn unregister_subscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
    ) -> bool {
        let Some(sessions) = self.subscriber_namespaces.get(track_namespace_prefix) else {
            return true;
        };

        sessions.remove(&session_id);
        let no_clients_remain = !sessions
            .iter()
            .any(|session| *session.value() == PeerKind::Client);
        let is_empty = sessions.is_empty();
        drop(sessions);

        if is_empty {
            self.subscriber_namespaces.remove(track_namespace_prefix);
        }

        no_clients_remain
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.register_publish",
        skip_all,
        fields(session_id = %session_id)
    )]
    pub(crate) async fn register_publish(
        &self,
        session_id: SessionId,
        handler: Arc<dyn PublishHandler>,
    ) {
        self.published_handlers
            .write()
            .await
            .push((session_id, handler));
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.get_namespace_subscribers",
        skip_all,
        fields(track_namespace = %track_namespace)
    )]
    pub(crate) fn get_namespace_subscribers(&self, track_namespace: &str) -> DashSet<SessionId> {
        let combined = DashSet::new();
        self.subscriber_namespaces
            .iter()
            // Check if the published namespace (track_namespace) falls under the subscribed prefix (entry.key())
            // Example: Published "room/member" starts with Subscribed "room" -> Match
            .filter(|entry| track_namespace.starts_with(entry.key()))
            .for_each(|entry| {
                entry.value().iter().for_each(|session| {
                    combined.insert(*session.key());
                })
            });
        combined
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.get_subscribers",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    #[allow(clippy::type_complexity)]
    pub(crate) async fn get_subscribers(
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
        name = "relay.local_pub_sub_directory.find_active_upstream_subscriptions",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) fn find_active_upstream_subscriptions(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Vec<UpstreamSubscriptionKey> {
        self.active_upstream_subscriptions
            .iter()
            .filter(|entry| {
                entry.key().track_namespace == track_namespace
                    && entry.key().track_name == track_name
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.find_upstream_publishers",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) async fn find_upstream_publishers(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Vec<UpstreamSubscriptionKey> {
        let publishers = DashSet::new();
        if let Some(namespace_publishers) = self.publisher_namespaces.get(track_namespace) {
            for session in namespace_publishers.iter() {
                publishers.insert(*session.key());
            }
        }

        let handlers = self.published_handlers.read().await;
        for session_id in handlers
            .iter()
            .filter(|(_, h)| h.track_namespace() == track_namespace && h.track_name() == track_name)
            .map(|(session_id, _)| *session_id)
        {
            publishers.insert(session_id);
        }
        publishers
            .into_iter()
            .map(|publisher_session_id| UpstreamSubscriptionKey {
                publisher_session_id,
                track_namespace: track_namespace.to_string(),
                track_name: track_name.to_string(),
            })
            .collect()
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.register_track_alias_link",
        skip_all,
        fields(
            publisher_session_id = %publisher_session_id,
            publisher_track_alias = %publisher_track_alias,
            subscriber_session_id = %subscriber_session_id,
            subscriber_track_alias = %subscriber_track_alias
        )
    )]
    pub(crate) fn register_track_alias_link(
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
        name = "relay.local_pub_sub_directory.find_subscriber_track_alias",
        skip_all,
        fields(
            publisher_session_id = %publisher_session_id,
            publisher_track_alias = %publisher_track_alias,
            subscriber_session_id = %subscriber_session_id
        )
    )]
    pub(crate) fn _find_subscriber_track_alias(
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

    pub(crate) fn get_active_upstream_subscription(
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

    /// Resolves the upstream subscription key linked to a downstream
    /// subscription identified by its Request ID, scoped to the given session.
    /// Used by Joining Fetch to find the subscription it joins.
    pub(crate) fn get_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<DownstreamSubscription> {
        self.downstream_subscriptions
            .get(&(downstream_session_id, downstream_subscribe_id))
            .map(|entry| entry.value().subscription.clone())
    }

    pub(crate) fn register_upstream_subscription(
        &self,
        key: UpstreamSubscriptionKey,
        subscription: ActiveUpstreamSubscription,
    ) {
        self.active_upstream_subscriptions.insert(key, subscription);
    }

    pub(crate) fn remove_upstream_subscription(
        &self,
        key: &UpstreamSubscriptionKey,
    ) -> Option<ActiveUpstreamSubscription> {
        self.active_upstream_subscriptions
            .remove(key)
            .map(|(_, subscription)| subscription)
    }

    /// Returns `None` when the upstream subscription is gone. The returned
    /// receiver resolves once the registration is removed, however that happens; the
    /// subscription's egress runner lives exactly until then.
    pub(crate) fn register_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        upstream_key: UpstreamSubscriptionKey,
        start_location: Option<moqt::Location>,
    ) -> Option<oneshot::Receiver<()>> {
        // The upstream entry stays locked until the registration is inserted: a concurrent removal
        // of the upstream either finds it or makes this registration fail. Lock order is always
        // upstream before downstream.
        let mut upstream = self.active_upstream_subscriptions.get_mut(&upstream_key)?;
        upstream.downstream_subscriber_count += 1;
        let (runner_stop_sender, runner_stop_receiver) = oneshot::channel();
        self.downstream_subscriptions.insert(
            (downstream_session_id, downstream_subscribe_id),
            RegisteredDownstreamSubscription {
                subscription: DownstreamSubscription {
                    upstream_key,
                    start_location,
                },
                _runner_stop_sender: runner_stop_sender,
            },
        );
        Some(runner_stop_receiver)
    }

    pub(crate) fn remove_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<RemovedDownstreamSubscription> {
        let (_, registered) = self
            .downstream_subscriptions
            .remove(&(downstream_session_id, downstream_subscribe_id))?;
        let upstream_key = registered.subscription.upstream_key;
        let Entry::Occupied(mut entry) = self
            .active_upstream_subscriptions
            .entry(upstream_key.clone())
        else {
            return None;
        };
        let upstream = entry.get_mut();
        if upstream.downstream_subscriber_count > 0 {
            upstream.downstream_subscriber_count -= 1;
        }
        let removed = RemovedDownstreamSubscription {
            upstream_key,
            upstream_request_id: upstream.upstream_request_id,
            track_key: upstream.track_key.clone(),
            remaining_downstream_subscriber_count: upstream.downstream_subscriber_count,
            upstream_origin: upstream.origin,
        };
        if upstream.downstream_subscriber_count == 0
            && upstream.origin == UpstreamSubscriptionOrigin::Subscribe
        {
            entry.remove();
        }
        Some(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::core::mocks::runner_stopped;
    use crate::modules::enums::{ContentExists, FilterType, GroupOrder};
    use crate::modules::types::TrackKey;

    #[derive(Debug)]
    struct StubPublishHandler {
        track_namespace: String,
        track_namespace_tuple: Vec<String>,
        track_name: String,
        track_alias: u64,
    }

    #[async_trait::async_trait]
    impl PublishHandler for StubPublishHandler {
        fn track_namespace(&self) -> &str {
            &self.track_namespace
        }

        fn track_namespace_tuple(&self) -> &[String] {
            &self.track_namespace_tuple
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

        fn _delivery_timeout(&self) -> Option<u64> {
            None
        }

        fn _max_cache_duration(&self) -> Option<u64> {
            None
        }

        fn subscription(
            &self,
            subscriber_priority: u8,
            filter_type: FilterType,
        ) -> crate::modules::core::subscription::UpstreamSubscription {
            crate::modules::core::subscription::UpstreamSubscription::from(
                moqt::PublisherInitiatedSubscription {
                    request_id: 0,
                    track_namespace: self.track_namespace.clone(),
                    track_name: self.track_name.clone(),
                    track_alias: self.track_alias,
                    group_order: moqt::GroupOrder::Ascending,
                    content_exists: moqt::ContentExists::False,
                    subscriber_priority,
                    forward: true,
                    filter_type: filter_type.as_moqt(),
                    delivery_timeout: None,
                },
            )
        }

        async fn ok(
            &self,
            _subscription: &crate::modules::core::subscription::UpstreamSubscription,
        ) -> Result<(), moqt::TransportSendError> {
            Ok(())
        }

        async fn accept_data_receiver(&self) {}

        async fn error(
            &self,
            _code: u64,
            _reason_phrase: String,
        ) -> Result<(), moqt::TransportSendError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn remove_session_cleans_up_all_session_scoped_entries() {
        // Arrange: Register namespace, track, and alias state for the session.
        let table = InMemoryLocalPubSubDirectory::new();

        assert!(table.register_publish_namespace(1, "room/member".to_string(), PeerKind::Client));
        assert!(table.register_publish_namespace(2, "room/member".to_string(), PeerKind::Client));
        table.register_subscribe_namespace(1, "room/".to_string(), PeerKind::Relay);
        table.register_subscribe_namespace(2, "room/".to_string(), PeerKind::Relay);
        table.register_subscribe_namespace(1, "solo/".to_string(), PeerKind::Client);
        table
            .register_publish(
                1,
                Arc::new(StubPublishHandler {
                    track_namespace: "room/member".to_string(),
                    track_namespace_tuple: vec!["room".to_string(), "member".to_string()],
                    track_name: "video".to_string(),
                    track_alias: 10,
                }),
            )
            .await;
        table.register_track_alias_link(1, 10, 2, 20);
        table.register_track_alias_link(2, 30, 1, 40);

        // Act: Remove all state associated with session 1.
        let removed = table.remove_session(1).await;

        // Assert: Remove only session 1 state while keeping other publishers in the same namespace.
        let upstream_subscriptions = table.find_upstream_publishers("room/member", "video").await;
        let publisher_session_ids: Vec<_> = upstream_subscriptions
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        assert_eq!(publisher_session_ids, vec![2]);
        assert!(table._find_subscriber_track_alias(1, 10, 2).is_none());
        assert!(table._find_subscriber_track_alias(2, 30, 1).is_none());

        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
        assert!(!room_subscribers.contains(&1));

        assert!(table.subscriber_namespaces.get("solo/").is_none());
        assert_eq!(
            removed.subscribe_namespace_prefixes,
            vec!["solo/".to_string()]
        );
        // Client publisher 2 still holds "room/member", so no Redis cleanup is requested.
        assert!(removed.publish_namespace_track_namespaces.is_empty());
    }

    #[tokio::test]
    async fn register_subscribe_namespace_reports_only_the_first_client() {
        // Arrange: Start with a relay subscriber, which never owns the route.
        let table = InMemoryLocalPubSubDirectory::new();

        // Act: Register a relay subscriber and then two client subscribers.
        let relay_is_first =
            table.register_subscribe_namespace(1, "room/".to_string(), PeerKind::Relay);
        let first_client =
            table.register_subscribe_namespace(2, "room/".to_string(), PeerKind::Client);
        let second_client =
            table.register_subscribe_namespace(3, "room/".to_string(), PeerKind::Client);

        // Assert: Only the first client registration requests route registration.
        assert!(!relay_is_first);
        assert!(first_client);
        assert!(!second_client);
    }

    #[tokio::test]
    async fn unregister_subscribe_namespace_reports_when_last_client_leaves() {
        // Arrange: Register two client subscribers for the same namespace prefix.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), PeerKind::Client);
        table.register_subscribe_namespace(2, "room/".to_string(), PeerKind::Client);

        // Act: Remove subscribers one by one.
        let still_has_clients = table.unregister_subscribe_namespace(1, "room/");
        let clients_became_empty = table.unregister_subscribe_namespace(2, "room/");

        // Assert: Only the final unsubscribe reports that no client remains.
        assert!(!still_has_clients);
        assert!(clients_became_empty);
        assert!(table.subscriber_namespaces.get("room/").is_none());
    }

    #[tokio::test]
    async fn unregister_subscribe_namespace_ignores_remaining_relay_subscribers() {
        // Arrange: Register a client subscriber alongside a relay subscriber.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), PeerKind::Client);
        table.register_subscribe_namespace(2, "room/".to_string(), PeerKind::Relay);

        // Act: Remove the final client subscriber while the relay subscriber remains.
        let clients_became_empty = table.unregister_subscribe_namespace(1, "room/");

        // Assert: The client-origin route can be cleaned up independently of relay subscribers.
        assert!(clients_became_empty);
        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
        assert!(!room_subscribers.contains(&1));
    }

    #[tokio::test]
    async fn remove_session_reports_empty_client_prefix_even_when_relay_subscriber_remains() {
        // Arrange: Register one client-origin subscriber and one relay-origin subscriber.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), PeerKind::Client);
        table.register_subscribe_namespace(2, "room/".to_string(), PeerKind::Relay);

        // Act: Disconnect the client-origin subscriber.
        let removed = table.remove_session(1).await;

        // Assert: Redis cleanup is requested while the relay subscriber stays registered locally.
        assert_eq!(
            removed.subscribe_namespace_prefixes,
            vec!["room/".to_string()]
        );
        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
        assert!(!room_subscribers.contains(&1));
    }

    #[tokio::test]
    async fn remove_session_does_not_report_relay_only_prefixes() {
        // Arrange: Register only relay-origin subscribers for the prefix.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), PeerKind::Relay);
        table.register_subscribe_namespace(2, "room/".to_string(), PeerKind::Relay);

        // Act: Disconnect one relay subscriber.
        let removed = table.remove_session(1).await;

        // Assert: No Redis cleanup is requested because no client ever owned the route.
        assert!(removed.subscribe_namespace_prefixes.is_empty());
        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
    }

    #[tokio::test]
    async fn remove_session_reports_publish_namespace_when_last_client_publisher_leaves() {
        // Arrange: Register one client-origin publisher and one relay-origin publisher.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), PeerKind::Client);
        table.register_publish_namespace(2, "room/member".to_string(), PeerKind::Relay);

        // Act: Disconnect the client-origin publisher.
        let removed = table.remove_session(1).await;

        // Assert: Redis cleanup is requested while the relay publisher stays registered locally.
        assert_eq!(
            removed.publish_namespace_track_namespaces,
            vec!["room/member".to_string()]
        );
        let publishers = table.find_upstream_publishers("room/member", "video").await;
        let publisher_session_ids: Vec<_> = publishers
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        assert_eq!(publisher_session_ids, vec![2]);
    }

    #[tokio::test]
    async fn remove_session_does_not_report_relay_only_publish_namespaces() {
        // Arrange: Register only relay-origin publishers for the namespace.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), PeerKind::Relay);
        table.register_publish_namespace(2, "room/member".to_string(), PeerKind::Relay);

        // Act: Disconnect one relay publisher.
        let removed = table.remove_session(1).await;

        // Assert: No Redis cleanup is requested because no client ever owned the route.
        assert!(removed.publish_namespace_track_namespaces.is_empty());
    }

    #[tokio::test]
    async fn purge_relay_publish_namespaces_drops_uncovered_relay_entries() {
        // Arrange: A relay-origin namespace learned over an inter-relay session,
        // alongside a local client publisher in another namespace.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(10, "research/ghost".to_string(), PeerKind::Relay);
        table.register_publish_namespace(1, "research/local".to_string(), PeerKind::Client);

        // Act: The last client subscriber for the prefix is gone.
        table.purge_relay_publish_namespaces("research");

        // Assert: The relay-origin ghost is dropped, the client publisher stays.
        assert!(table.publisher_namespaces.get("research/ghost").is_none());
        assert!(table.publisher_namespaces.get("research/local").is_some());
    }

    #[tokio::test]
    async fn purge_relay_publish_namespaces_keeps_entries_covered_by_client_prefix() {
        // Arrange: A relay-origin namespace still watched via another client prefix.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(10, "research/ghost".to_string(), PeerKind::Relay);
        table.register_subscribe_namespace(2, "research".to_string(), PeerKind::Client);

        // Act: Purge for the same prefix while the client subscriber remains.
        table.purge_relay_publish_namespaces("research");

        // Assert: The covered namespace must not be purged.
        assert!(table.publisher_namespaces.get("research/ghost").is_some());
    }

    #[tokio::test]
    async fn unregister_publish_namespace_reports_when_last_client_leaves() {
        // Arrange: Register two client publishers for the same namespace.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), PeerKind::Client);
        table.register_publish_namespace(2, "room/member".to_string(), PeerKind::Client);

        // Act: Remove publishers one by one.
        let still_has_clients = table.unregister_publish_namespace(1, "room/member");
        let clients_became_empty = table.unregister_publish_namespace(2, "room/member");

        // Assert: Only the final withdrawal reports that no client remains.
        assert!(!still_has_clients);
        assert!(clients_became_empty);
        assert!(table.publisher_namespaces.get("room/member").is_none());
    }

    #[tokio::test]
    async fn unregister_publish_namespace_ignores_remaining_relay_publishers() {
        // Arrange: Register a client publisher alongside a relay publisher.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), PeerKind::Client);
        table.register_publish_namespace(2, "room/member".to_string(), PeerKind::Relay);

        // Act: Withdraw the final client publisher while the relay publisher remains.
        let clients_became_empty = table.unregister_publish_namespace(1, "room/member");

        // Assert: Route cleanup is allowed while the relay publisher stays registered.
        assert!(clients_became_empty);
        let publishers = table.find_upstream_publishers("room/member", "video").await;
        let publisher_session_ids: Vec<_> = publishers
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        assert_eq!(publisher_session_ids, vec![2]);
    }

    #[tokio::test]
    async fn allows_multiple_publishers_for_the_same_namespace_and_track() {
        // Arrange: Register multiple publishers for the same namespace and track.
        let table = InMemoryLocalPubSubDirectory::new();

        table.register_publish_namespace(1, "room/member".to_string(), PeerKind::Client);
        table.register_publish_namespace(2, "room/member".to_string(), PeerKind::Client);
        table
            .register_publish(
                1,
                Arc::new(StubPublishHandler {
                    track_namespace: "room/member".to_string(),
                    track_namespace_tuple: vec!["room".to_string(), "member".to_string()],
                    track_name: "video".to_string(),
                    track_alias: 10,
                }),
            )
            .await;
        table
            .register_publish(
                2,
                Arc::new(StubPublishHandler {
                    track_namespace: "room/member".to_string(),
                    track_namespace_tuple: vec!["room".to_string(), "member".to_string()],
                    track_name: "video".to_string(),
                    track_alias: 20,
                }),
            )
            .await;

        // Act: Find upstream publishers available for subscribe.
        let mut upstream_publishers: Vec<_> = table
            .find_upstream_publishers("room/member", "video")
            .await
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        upstream_publishers.sort();

        // Assert: Keep multiple publishers regardless of whether they came from namespace or track state.
        assert_eq!(upstream_publishers, vec![1, 2]);
    }

    fn subscribed_track_table() -> (InMemoryLocalPubSubDirectory, UpstreamSubscriptionKey) {
        let table = InMemoryLocalPubSubDirectory::new();
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id: 1,
            track_namespace: "ns".to_string(),
            track_name: "track".to_string(),
        };
        table.register_upstream_subscription(
            upstream_key.clone(),
            ActiveUpstreamSubscription {
                upstream_request_id: 1,
                track_key: TrackKey::new("ns", "track"),
                expires: None,
                content_exists: ContentExists::False,
                downstream_subscriber_count: 0,
                origin: UpstreamSubscriptionOrigin::Subscribe,
            },
        );
        (table, upstream_key)
    }

    #[tokio::test]
    async fn register_downstream_subscription_stores_start_location() {
        // Arrange
        let (table, upstream_key) = subscribed_track_table();
        let largest = moqt::Location {
            group_id: 5,
            object_id: 3,
        };

        // Act
        let runner_stop_receiver =
            table.register_downstream_subscription(2, 100, upstream_key.clone(), Some(largest));

        // Assert
        assert!(runner_stop_receiver.is_some());
        let sub = table.get_downstream_subscription(2, 100).unwrap();
        assert_eq!(sub.upstream_key, upstream_key);
        assert_eq!(
            sub.start_location,
            Some(moqt::Location {
                group_id: 5,
                object_id: 3
            })
        );
    }

    #[tokio::test]
    async fn register_downstream_subscription_none_start_location() {
        // Arrange
        let (table, upstream_key) = subscribed_track_table();

        // Act
        let runner_stop_receiver =
            table.register_downstream_subscription(2, 100, upstream_key.clone(), None);

        // Assert
        assert!(runner_stop_receiver.is_some());
        let sub = table.get_downstream_subscription(2, 100).unwrap();
        assert_eq!(sub.upstream_key, upstream_key);
        assert!(sub.start_location.is_none());
    }

    #[tokio::test]
    async fn publisher_disconnect_stops_the_runners_of_its_downstream_subscriptions() {
        // Arrange
        let (table, upstream_key) = subscribed_track_table();
        let mut runner_stop_receiver = table
            .register_downstream_subscription(2, 100, upstream_key, None)
            .unwrap();

        // Act
        table.remove_session(1).await;

        // Assert
        assert!(runner_stopped(&mut runner_stop_receiver));
    }

    #[tokio::test]
    async fn subscriber_disconnect_after_malformed_cleanup_stops_its_runner() {
        // Arrange
        let (table, upstream_key) = subscribed_track_table();
        let mut runner_stop_receiver = table
            .register_downstream_subscription(2, 100, upstream_key.clone(), None)
            .unwrap();
        table.remove_upstream_subscription(&upstream_key).unwrap();

        // Act
        table.remove_session(2).await;

        // Assert
        assert!(runner_stopped(&mut runner_stop_receiver));
        assert!(table.downstream_subscriptions.is_empty());
    }

    #[tokio::test]
    async fn registration_for_a_removed_upstream_yields_no_runner() {
        // Arrange
        let (table, upstream_key) = subscribed_track_table();
        table.remove_session(1).await;

        // Act
        let runner_stop_receiver =
            table.register_downstream_subscription(2, 100, upstream_key, None);

        // Assert
        assert!(runner_stop_receiver.is_none());
        assert!(table.downstream_subscriptions.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registration_racing_publisher_removal_leaves_no_orphan() {
        for _ in 0..2000 {
            // Arrange
            let (table, upstream_key) = subscribed_track_table();
            let table = Arc::new(table);
            let barrier = Arc::new(tokio::sync::Barrier::new(2));

            // Act
            let register = tokio::spawn({
                let table = table.clone();
                let barrier = barrier.clone();
                async move {
                    barrier.wait().await;
                    table
                        .register_downstream_subscription(2, 100, upstream_key, None)
                        .is_some()
                }
            });
            let remove = tokio::spawn({
                let table = table.clone();
                async move {
                    barrier.wait().await;
                    table.remove_session(1).await
                }
            });
            let registered = register.await.unwrap();
            let removed = remove.await.unwrap();

            // Assert
            let reported = !removed.downstream_subscriptions.is_empty();
            assert_eq!(registered, reported);
            assert!(table.downstream_subscriptions.is_empty());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registration_racing_last_subscriber_removal_leaves_no_orphan() {
        for _ in 0..20000 {
            // Arrange
            let (table, upstream_key) = subscribed_track_table();
            table
                .register_downstream_subscription(2, 100, upstream_key.clone(), None)
                .unwrap();
            let table = Arc::new(table);
            let barrier = Arc::new(tokio::sync::Barrier::new(2));

            // Act
            let register = tokio::spawn({
                let table = table.clone();
                let barrier = barrier.clone();
                let upstream_key = upstream_key.clone();
                async move {
                    barrier.wait().await;
                    table
                        .register_downstream_subscription(3, 200, upstream_key, None)
                        .is_some()
                }
            });
            let remove = tokio::spawn({
                let table = table.clone();
                async move {
                    barrier.wait().await;
                    table.remove_downstream_subscription(2, 100)
                }
            });
            let registered = register.await.unwrap();
            let removed = remove.await.unwrap().unwrap();

            // Assert
            let upstream_exists = table
                .active_upstream_subscriptions
                .contains_key(&upstream_key);
            assert_eq!(registered, upstream_exists);
            assert_eq!(
                removed.remaining_downstream_subscriber_count,
                usize::from(registered)
            );
            assert_eq!(
                table.downstream_subscriptions.len(),
                usize::from(registered)
            );
        }
    }

    #[tokio::test]
    async fn finds_active_upstream_subscriptions_separately_from_publishers() {
        // Arrange: Register an active upstream subscription separately from publishers.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), PeerKind::Client);
        let upstream_key = UpstreamSubscriptionKey {
            publisher_session_id: 1,
            track_namespace: "room/member".to_string(),
            track_name: "video".to_string(),
        };
        table.register_upstream_subscription(
            upstream_key.clone(),
            ActiveUpstreamSubscription {
                upstream_request_id: 10,
                track_key: TrackKey::new("room/member", "video"),
                expires: Some(30),
                content_exists: ContentExists::False,
                downstream_subscriber_count: 1,
                origin: UpstreamSubscriptionOrigin::Subscribe,
            },
        );

        // Act: Fetch the active upstream subscription and upstream publishers separately.
        let active_subscriptions = table.find_active_upstream_subscriptions("room/member", "video");
        let publisher_subscriptions = table.find_upstream_publishers("room/member", "video").await;

        // Assert: Active subscriptions and upstream publishers are both discoverable.
        assert_eq!(active_subscriptions, vec![upstream_key.clone()]);
        assert_eq!(publisher_subscriptions, vec![upstream_key]);
    }
}

pub(crate) mod entry;

use std::{
    collections::HashSet,
    sync::{Arc, PoisonError, RwLock},
};

use dashmap::{DashMap, DashSet, Entry};
use tokio::sync::{oneshot, watch};

use crate::modules::{
    domain::{
        pub_sub_directory::entry::{
            ActiveUpstreamSubscription, DownstreamSubscription, PublishDoneReason,
            ReleasedUpstreamSubscription, RemovedDownstreamSubscription,
            RemovedSessionSubscriptions, UpstreamSubscriptionKey, UpstreamSubscriptionOrigin,
            UpstreamTrack,
        },
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::{TrackKey, TrackNamespace, TrackNamespacePrefix},
    },
    session::handler::publish::PublishHandler,
};

pub(crate) struct RegisteredDownstreamSubscription {
    pub(crate) subscription: DownstreamSubscription,
    runner_stop_sender: oneshot::Sender<PublishDoneReason>,
    forward_sender: watch::Sender<bool>,
}

pub(crate) struct DownstreamRunnerSignals {
    pub(crate) stop_receiver: oneshot::Receiver<PublishDoneReason>,
    pub(crate) forward_receiver: watch::Receiver<bool>,
}

// Client subscriptions own the Redis route for their prefix, so the
// directory tracks the peer to detect when the last client leaves.
type PeersByNamespace = DashMap<String, DashMap<SessionId, SessionPeer>>;

fn has_client(peers: &DashMap<SessionId, SessionPeer>) -> bool {
    peers
        .iter()
        .any(|peer| *peer.value() == SessionPeer::Client)
}

/// Returns the namespaces whose last client was the removed session; relay
/// peers don't own Redis routes, so only those need route cleanup.
fn remove_peer_from_namespaces(
    namespaces: &PeersByNamespace,
    session_id: SessionId,
) -> Vec<String> {
    let mut last_client_left = Vec::new();
    let mut empty_namespaces = Vec::new();
    for entry in namespaces.iter() {
        let removed_kind = entry.value().remove(&session_id).map(|(_, kind)| kind);
        if removed_kind == Some(SessionPeer::Client) && !has_client(entry.value()) {
            last_client_left.push(entry.key().clone());
        }
        if entry.value().is_empty() {
            empty_namespaces.push(entry.key().clone());
        }
    }
    for namespace in empty_namespaces {
        namespaces.remove(&namespace);
    }
    last_client_left
}

fn unregister_peer(namespaces: &PeersByNamespace, session_id: SessionId, namespace: &str) -> bool {
    let Some(peers) = namespaces.get(namespace) else {
        return true;
    };

    peers.remove(&session_id);
    let no_clients_remain = !has_client(&peers);
    let is_empty = peers.is_empty();
    drop(peers);

    if is_empty {
        namespaces.remove(namespace);
    }

    no_clients_remain
}

pub(crate) struct InMemoryLocalPubSubDirectory {
    pub(crate) publisher_namespaces: DashMap<TrackNamespace, DashMap<SessionId, SessionPeer>>,
    pub(crate) subscriber_namespaces:
        DashMap<TrackNamespacePrefix, DashMap<SessionId, SessionPeer>>,
    pub(crate) published_handlers: RwLock<Vec<(SessionId, Arc<dyn PublishHandler>)>>,
    pub(crate) upstream_tracks: DashMap<TrackKey, UpstreamTrack>,
    pub(crate) downstream_subscriptions:
        DashMap<(SessionId, u64), RegisteredDownstreamSubscription>,
}

impl InMemoryLocalPubSubDirectory {
    pub(crate) fn new() -> Self {
        Self {
            publisher_namespaces: DashMap::new(),
            subscriber_namespaces: DashMap::new(),
            published_handlers: RwLock::new(Vec::new()),
            upstream_tracks: DashMap::new(),
            downstream_subscriptions: DashMap::new(),
        }
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.remove_session",
        skip_all,
        fields(session_id = %session_id)
    )]
    pub(crate) fn remove_session(&self, session_id: SessionId) -> RemovedSessionSubscriptions {
        let mut removed = RemovedSessionSubscriptions {
            publish_namespace_track_namespaces: remove_peer_from_namespaces(
                &self.publisher_namespaces,
                session_id,
            ),
            subscribe_namespace_prefixes: remove_peer_from_namespaces(
                &self.subscriber_namespaces,
                session_id,
            ),
            ..Default::default()
        };

        self.published_handlers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(registered_session_id, _)| *registered_session_id != session_id);

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

        let published_track_keys: Vec<_> = self
            .upstream_tracks
            .iter()
            .filter_map(|entry| {
                entry
                    .value()
                    .subscriptions
                    .contains_key(&session_id)
                    .then(|| entry.key().clone())
            })
            .collect();
        for track_key in published_track_keys {
            let Some((_, downstream_subscriptions)) = self.end_publisher_subscription(
                &track_key,
                session_id,
                PublishDoneReason::publisher_session_closed(),
            ) else {
                continue;
            };
            removed.upstream_track_keys.push(track_key);
            removed
                .downstream_subscriptions
                .extend(downstream_subscriptions);
        }

        removed
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.register_publish_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace, peer = ?peer)
    )]
    pub(crate) fn register_publish_namespace(
        &self,
        session_id: SessionId,
        track_namespace: String,
        peer: SessionPeer,
    ) {
        if let Some(sessions) = self.publisher_namespaces.get_mut(&track_namespace) {
            sessions.insert(session_id, peer);
        } else {
            let sessions = DashMap::new();
            sessions.insert(session_id, peer);
            self.publisher_namespaces.insert(track_namespace, sessions);
        }
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
        unregister_peer(&self.publisher_namespaces, session_id, track_namespace)
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
                entry.key().starts_with(prefix_entry.key()) && has_client(prefix_entry.value())
            });
            if covered {
                continue;
            }

            let relay_session_ids: Vec<SessionId> = entry
                .value()
                .iter()
                .filter(|session| *session.value() == SessionPeer::Relay)
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
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix, peer = ?peer)
    )]
    pub(crate) fn register_subscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: String,
        peer: SessionPeer,
    ) -> bool {
        if let Some(sessions) = self.subscriber_namespaces.get_mut(&track_namespace_prefix) {
            let had_client = has_client(&sessions);
            sessions.insert(session_id, peer);
            peer == SessionPeer::Client && !had_client
        } else {
            tracing::info!(
                session_id = %session_id,
                track_namespace_prefix = %track_namespace_prefix,
                "New namespace prefix is subscribed."
            );
            let sessions = DashMap::new();
            sessions.insert(session_id, peer);
            self.subscriber_namespaces
                .insert(track_namespace_prefix, sessions);
            peer == SessionPeer::Client
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
        unregister_peer(
            &self.subscriber_namespaces,
            session_id,
            track_namespace_prefix,
        )
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.register_publish",
        skip_all,
        fields(session_id = %session_id)
    )]
    pub(crate) fn register_publish(&self, session_id: SessionId, handler: Arc<dyn PublishHandler>) {
        self.published_handlers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
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
    pub(crate) fn get_subscribers(
        &self,
        track_namespace_prefix: &str,
    ) -> HashSet<(String, Option<(String, u64)>)> {
        let mut filtered = HashSet::new();
        for entry in self.publisher_namespaces.iter() {
            if entry.key().starts_with(track_namespace_prefix) {
                filtered.insert((entry.key().clone(), None));
            }
        }

        for (_, handler) in self
            .published_handlers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            if handler
                .track_namespace()
                .starts_with(track_namespace_prefix)
            {
                filtered.insert((
                    handler.track_namespace().to_string(),
                    Some((handler.track_name().to_string(), handler.track_alias())),
                ));
            }
        }
        filtered
    }

    #[tracing::instrument(
        level = "info",
        name = "relay.local_pub_sub_directory.find_upstream_publishers",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    pub(crate) fn find_upstream_publishers(
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

        let handlers = self
            .published_handlers
            .read()
            .unwrap_or_else(PoisonError::into_inner);
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

    pub(crate) fn get_upstream_track(&self, track_key: &TrackKey) -> Option<UpstreamTrack> {
        self.upstream_tracks
            .get(track_key)
            .map(|entry| entry.value().clone())
    }

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
        track_key: TrackKey,
        publisher_session_id: SessionId,
        subscription: ActiveUpstreamSubscription,
    ) {
        self.upstream_tracks
            .entry(track_key)
            .or_default()
            .subscriptions
            .insert(publisher_session_id, subscription);
    }

    /// Leaves the downstream registrations of a track whose last upstream
    /// subscription this removes to their subscribers: a malformed track ends
    /// their runners on its own.
    pub(crate) fn remove_upstream_subscription(
        &self,
        key: &UpstreamSubscriptionKey,
    ) -> Option<ActiveUpstreamSubscription> {
        let track_key = TrackKey::new(&key.track_namespace, &key.track_name);
        let Entry::Occupied(mut track) = self.upstream_tracks.entry(track_key) else {
            return None;
        };
        let removed = track
            .get_mut()
            .subscriptions
            .remove(&key.publisher_session_id);
        if track.get().subscriptions.is_empty() {
            track.remove();
        }
        removed
    }

    pub(crate) fn end_upstream_subscription(
        &self,
        publisher_session_id: SessionId,
        upstream_request_id: u64,
        end: PublishDoneReason,
    ) -> Option<TrackKey> {
        let track_key = self
            .upstream_tracks
            .iter()
            .find(|entry| {
                entry
                    .value()
                    .subscriptions
                    .get(&publisher_session_id)
                    .is_some_and(|subscription| {
                        subscription.upstream_request_id == upstream_request_id
                    })
            })
            .map(|entry| entry.key().clone())?;
        let (ended_subscription, _) =
            self.end_publisher_subscription(&track_key, publisher_session_id, end)?;
        if ended_subscription.origin == UpstreamSubscriptionOrigin::Publish {
            self.published_handlers
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|(session_id, handler)| {
                    *session_id != publisher_session_id
                        || handler.track_namespace() != track_key.track_namespace
                        || handler.track_name() != track_key.track_name
                });
        }
        Some(track_key)
    }

    /// The track ends with its last upstream subscription: its downstream
    /// subscriptions are removed and their runners told why.
    fn end_publisher_subscription(
        &self,
        track_key: &TrackKey,
        publisher_session_id: SessionId,
        end: PublishDoneReason,
    ) -> Option<(
        ActiveUpstreamSubscription,
        Vec<RemovedDownstreamSubscription>,
    )> {
        let Entry::Occupied(mut track) = self.upstream_tracks.entry(track_key.clone()) else {
            return None;
        };
        let ended_subscription = track
            .get_mut()
            .subscriptions
            .remove(&publisher_session_id)?;
        if !track.get().subscriptions.is_empty() {
            return Some((ended_subscription, Vec::new()));
        }
        track.remove();
        let downstream_keys: Vec<_> = self
            .downstream_subscriptions
            .iter()
            .filter_map(|entry| {
                (&entry.value().subscription.track_key == track_key).then_some(*entry.key())
            })
            .collect();
        let mut removed = Vec::new();
        for downstream_key in downstream_keys {
            let Some((_, registered)) = self.downstream_subscriptions.remove(&downstream_key)
            else {
                continue;
            };
            let _ = registered.runner_stop_sender.send(end.clone());
            removed.push(RemovedDownstreamSubscription {
                track_key: track_key.clone(),
                released_upstream_subscriptions: Vec::new(),
            });
        }
        Some((ended_subscription, removed))
    }

    /// Returns `None` when the upstream track is gone. The returned
    /// stop receiver resolves once the registration is removed, however that happens; the
    /// subscription's egress runner lives exactly until then. The forward receiver
    /// starts at Forward State 1 whatever the SUBSCRIBE asked for.
    pub(crate) fn register_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        track_key: TrackKey,
        start_location: Option<moqt::Location>,
    ) -> Option<DownstreamRunnerSignals> {
        // The track entry stays locked until the registration is inserted: a concurrent removal
        // of the track either finds it or makes this registration fail. Lock order is always
        // upstream before downstream.
        let mut upstream = self.upstream_tracks.get_mut(&track_key)?;
        upstream.downstream_subscriber_count += 1;
        let (runner_stop_sender, stop_receiver) = oneshot::channel();
        let (forward_sender, forward_receiver) = watch::channel(true);
        self.downstream_subscriptions.insert(
            (downstream_session_id, downstream_subscribe_id),
            RegisteredDownstreamSubscription {
                subscription: DownstreamSubscription {
                    track_key,
                    start_location,
                },
                runner_stop_sender,
                forward_sender,
            },
        );
        Some(DownstreamRunnerSignals {
            stop_receiver,
            forward_receiver,
        })
    }

    /// Returns false when no such downstream subscription is registered.
    pub(crate) fn update_downstream_forward(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        forward: bool,
    ) -> bool {
        let Some(registered) = self
            .downstream_subscriptions
            .get(&(downstream_session_id, downstream_subscribe_id))
        else {
            return false;
        };
        registered.forward_sender.send_replace(forward);
        true
    }

    /// The last downstream subscriber leaving releases the track's
    /// SUBSCRIBE-initiated upstream subscriptions; PUBLISH-initiated ones stay
    /// until their publisher ends them.
    pub(crate) fn remove_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<RemovedDownstreamSubscription> {
        let (_, registered) = self
            .downstream_subscriptions
            .remove(&(downstream_session_id, downstream_subscribe_id))?;
        let track_key = registered.subscription.track_key;
        let Entry::Occupied(mut entry) = self.upstream_tracks.entry(track_key.clone()) else {
            return None;
        };
        let track = entry.get_mut();
        track.downstream_subscriber_count = track.downstream_subscriber_count.saturating_sub(1);
        let mut released_upstream_subscriptions = Vec::new();
        if track.downstream_subscriber_count == 0 {
            track
                .subscriptions
                .retain(|publisher_session_id, subscription| {
                    let released = subscription.origin == UpstreamSubscriptionOrigin::Subscribe;
                    if released {
                        released_upstream_subscriptions.push(ReleasedUpstreamSubscription {
                            publisher_session_id: *publisher_session_id,
                            upstream_request_id: subscription.upstream_request_id,
                        });
                    }
                    !released
                });
        }
        if track.subscriptions.is_empty() {
            entry.remove();
        }
        Some(RemovedDownstreamSubscription {
            track_key,
            released_upstream_subscriptions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::directory_fixtures::{
        PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, table_with_upstream, upstream_key,
    };
    use crate::modules::test_support::mock_session::runner_stopped;
    use moqt::{ContentExists, FilterType, GroupOrder};

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
        ) -> crate::modules::session::subscription::UpstreamSubscription {
            crate::modules::session::subscription::UpstreamSubscription::from(
                moqt::PublisherInitiatedSubscription {
                    request_id: 0,
                    track_namespace: self.track_namespace.clone(),
                    track_name: self.track_name.clone(),
                    track_alias: self.track_alias,
                    group_order: GroupOrder::Ascending,
                    content_exists: ContentExists::False,
                    subscriber_priority,
                    forward: true,
                    filter_type,
                    delivery_timeout: None,
                },
            )
        }

        async fn ok(
            &self,
            _subscription: &crate::modules::session::subscription::UpstreamSubscription,
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

    #[test]
    fn remove_session_cleans_up_all_session_scoped_entries() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();

        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        table.register_publish_namespace(2, "room/member".to_string(), SessionPeer::Client);
        table.register_subscribe_namespace(1, "room/".to_string(), SessionPeer::Relay);
        table.register_subscribe_namespace(2, "room/".to_string(), SessionPeer::Relay);
        table.register_subscribe_namespace(1, "solo/".to_string(), SessionPeer::Client);
        table.register_publish(
            1,
            Arc::new(StubPublishHandler {
                track_namespace: "room/member".to_string(),
                track_namespace_tuple: vec!["room".to_string(), "member".to_string()],
                track_name: "video".to_string(),
                track_alias: 10,
            }),
        );

        // Act: Remove all state associated with session 1.
        let removed = table.remove_session(1);

        // Assert: Remove only session 1 state while keeping other publishers in the same namespace.
        let upstream_subscriptions = table.find_upstream_publishers("room/member", "video");
        let publisher_session_ids: Vec<_> = upstream_subscriptions
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        assert_eq!(publisher_session_ids, vec![2]);

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

    #[test]
    fn register_subscribe_namespace_reports_only_the_first_client() {
        // Arrange: Start with a relay subscriber, which never owns the route.
        let table = InMemoryLocalPubSubDirectory::new();

        // Act: Register a relay subscriber and then two client subscribers.
        let relay_is_first =
            table.register_subscribe_namespace(1, "room/".to_string(), SessionPeer::Relay);
        let first_client =
            table.register_subscribe_namespace(2, "room/".to_string(), SessionPeer::Client);
        let second_client =
            table.register_subscribe_namespace(3, "room/".to_string(), SessionPeer::Client);

        // Assert: Only the first client registration requests route registration.
        assert!(!relay_is_first);
        assert!(first_client);
        assert!(!second_client);
    }

    #[test]
    fn unregister_subscribe_namespace_reports_when_last_client_leaves() {
        // Arrange: Register two client subscribers for the same namespace prefix.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), SessionPeer::Client);
        table.register_subscribe_namespace(2, "room/".to_string(), SessionPeer::Client);

        // Act: Remove subscribers one by one.
        let still_has_clients = table.unregister_subscribe_namespace(1, "room/");
        let clients_became_empty = table.unregister_subscribe_namespace(2, "room/");

        // Assert: Only the final unsubscribe reports that no client remains.
        assert!(!still_has_clients);
        assert!(clients_became_empty);
        assert!(table.subscriber_namespaces.get("room/").is_none());
    }

    #[test]
    fn unregister_subscribe_namespace_ignores_remaining_relay_subscribers() {
        // Arrange: Register a client subscriber alongside a relay subscriber.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), SessionPeer::Client);
        table.register_subscribe_namespace(2, "room/".to_string(), SessionPeer::Relay);

        // Act: Remove the final client subscriber while the relay subscriber remains.
        let clients_became_empty = table.unregister_subscribe_namespace(1, "room/");

        // Assert: The client-origin route can be cleaned up independently of relay subscribers.
        assert!(clients_became_empty);
        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
        assert!(!room_subscribers.contains(&1));
    }

    #[test]
    fn remove_session_reports_empty_client_prefix_even_when_relay_subscriber_remains() {
        // Arrange: Register one client-origin subscriber and one relay-origin subscriber.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), SessionPeer::Client);
        table.register_subscribe_namespace(2, "room/".to_string(), SessionPeer::Relay);

        // Act: Disconnect the client-origin subscriber.
        let removed = table.remove_session(1);

        // Assert: Redis cleanup is requested while the relay subscriber stays registered locally.
        assert_eq!(
            removed.subscribe_namespace_prefixes,
            vec!["room/".to_string()]
        );
        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
        assert!(!room_subscribers.contains(&1));
    }

    #[test]
    fn remove_session_does_not_report_relay_only_prefixes() {
        // Arrange: Register only relay-origin subscribers for the prefix.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "room/".to_string(), SessionPeer::Relay);
        table.register_subscribe_namespace(2, "room/".to_string(), SessionPeer::Relay);

        // Act: Disconnect one relay subscriber.
        let removed = table.remove_session(1);

        // Assert: No Redis cleanup is requested because no client ever owned the route.
        assert!(removed.subscribe_namespace_prefixes.is_empty());
        let room_subscribers = table.get_namespace_subscribers("room/member");
        assert!(room_subscribers.contains(&2));
    }

    #[test]
    fn remove_session_reports_publish_namespace_when_last_client_publisher_leaves() {
        // Arrange: Register one client-origin publisher and one relay-origin publisher.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        table.register_publish_namespace(2, "room/member".to_string(), SessionPeer::Relay);

        // Act: Disconnect the client-origin publisher.
        let removed = table.remove_session(1);

        // Assert: Redis cleanup is requested while the relay publisher stays registered locally.
        assert_eq!(
            removed.publish_namespace_track_namespaces,
            vec!["room/member".to_string()]
        );
        let publishers = table.find_upstream_publishers("room/member", "video");
        let publisher_session_ids: Vec<_> = publishers
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        assert_eq!(publisher_session_ids, vec![2]);
    }

    #[test]
    fn remove_session_does_not_report_relay_only_publish_namespaces() {
        // Arrange: Register only relay-origin publishers for the namespace.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Relay);
        table.register_publish_namespace(2, "room/member".to_string(), SessionPeer::Relay);

        // Act: Disconnect one relay publisher.
        let removed = table.remove_session(1);

        // Assert: No Redis cleanup is requested because no client ever owned the route.
        assert!(removed.publish_namespace_track_namespaces.is_empty());
    }

    #[test]
    fn purge_relay_publish_namespaces_drops_uncovered_relay_entries() {
        // Arrange: A relay-origin namespace learned over an inter-relay session,
        // alongside a local client publisher in another namespace.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(10, "research/ghost".to_string(), SessionPeer::Relay);
        table.register_publish_namespace(1, "research/local".to_string(), SessionPeer::Client);

        // Act: The last client subscriber for the prefix is gone.
        table.purge_relay_publish_namespaces("research");

        // Assert: The relay-origin ghost is dropped, the client publisher stays.
        assert!(table.publisher_namespaces.get("research/ghost").is_none());
        assert!(table.publisher_namespaces.get("research/local").is_some());
    }

    #[test]
    fn purge_relay_publish_namespaces_keeps_entries_covered_by_client_prefix() {
        // Arrange: A relay-origin namespace still watched via another client prefix.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(10, "research/ghost".to_string(), SessionPeer::Relay);
        table.register_subscribe_namespace(2, "research".to_string(), SessionPeer::Client);

        // Act: Purge for the same prefix while the client subscriber remains.
        table.purge_relay_publish_namespaces("research");

        // Assert: The covered namespace must not be purged.
        assert!(table.publisher_namespaces.get("research/ghost").is_some());
    }

    #[test]
    fn unregister_publish_namespace_reports_when_last_client_leaves() {
        // Arrange: Register two client publishers for the same namespace.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        table.register_publish_namespace(2, "room/member".to_string(), SessionPeer::Client);

        // Act: Remove publishers one by one.
        let still_has_clients = table.unregister_publish_namespace(1, "room/member");
        let clients_became_empty = table.unregister_publish_namespace(2, "room/member");

        // Assert: Only the final withdrawal reports that no client remains.
        assert!(!still_has_clients);
        assert!(clients_became_empty);
        assert!(table.publisher_namespaces.get("room/member").is_none());
    }

    #[test]
    fn unregister_publish_namespace_ignores_remaining_relay_publishers() {
        // Arrange: Register a client publisher alongside a relay publisher.
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        table.register_publish_namespace(2, "room/member".to_string(), SessionPeer::Relay);

        // Act: Withdraw the final client publisher while the relay publisher remains.
        let clients_became_empty = table.unregister_publish_namespace(1, "room/member");

        // Assert: Route cleanup is allowed while the relay publisher stays registered.
        assert!(clients_became_empty);
        let publishers = table.find_upstream_publishers("room/member", "video");
        let publisher_session_ids: Vec<_> = publishers
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        assert_eq!(publisher_session_ids, vec![2]);
    }

    #[test]
    fn allows_multiple_publishers_for_the_same_namespace_and_track() {
        // Arrange: Register multiple publishers for the same namespace and track.
        let table = InMemoryLocalPubSubDirectory::new();

        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        table.register_publish_namespace(2, "room/member".to_string(), SessionPeer::Client);
        table.register_publish(
            1,
            Arc::new(StubPublishHandler {
                track_namespace: "room/member".to_string(),
                track_namespace_tuple: vec!["room".to_string(), "member".to_string()],
                track_name: "video".to_string(),
                track_alias: 10,
            }),
        );
        table.register_publish(
            2,
            Arc::new(StubPublishHandler {
                track_namespace: "room/member".to_string(),
                track_namespace_tuple: vec!["room".to_string(), "member".to_string()],
                track_name: "video".to_string(),
                track_alias: 20,
            }),
        );

        // Act: Find upstream publishers available for subscribe.
        let mut upstream_publishers: Vec<_> = table
            .find_upstream_publishers("room/member", "video")
            .into_iter()
            .map(|subscription| subscription.publisher_session_id)
            .collect();
        upstream_publishers.sort();

        // Assert: Keep multiple publishers regardless of whether they came from namespace or track state.
        assert_eq!(upstream_publishers, vec![1, 2]);
    }

    #[test]
    fn register_downstream_subscription_stores_start_location() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let largest = moqt::Location {
            group_id: 5,
            object_id: 3,
        };

        // Act
        let runner_signals =
            table.register_downstream_subscription(2, 100, track_key.clone(), Some(largest));

        // Assert
        assert!(runner_signals.is_some());
        let sub = table.get_downstream_subscription(2, 100).unwrap();
        assert_eq!(sub.track_key, track_key);
        assert_eq!(
            sub.start_location,
            Some(moqt::Location {
                group_id: 5,
                object_id: 3
            })
        );
    }

    #[test]
    fn register_downstream_subscription_none_start_location() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);

        // Act
        let runner_signals =
            table.register_downstream_subscription(2, 100, track_key.clone(), None);

        // Assert
        assert!(runner_signals.is_some());
        let sub = table.get_downstream_subscription(2, 100).unwrap();
        assert_eq!(sub.track_key, track_key);
        assert!(sub.start_location.is_none());
    }

    #[test]
    fn registered_subscription_starts_forwarding() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);

        // Act
        let signals = table
            .register_downstream_subscription(2, 100, track_key, None)
            .unwrap();

        // Assert
        assert!(*signals.forward_receiver.borrow());
    }

    #[test]
    fn forward_update_reaches_the_registered_runner() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let signals = table
            .register_downstream_subscription(2, 100, track_key, None)
            .unwrap();

        // Act
        let updated = table.update_downstream_forward(2, 100, false);

        // Assert
        assert!(updated);
        assert!(!*signals.forward_receiver.borrow());
    }

    #[test]
    fn forward_update_for_an_unknown_subscription_is_reported() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let _signals = table.register_downstream_subscription(2, 100, track_key, None);

        // Act
        let updated = table.update_downstream_forward(2, 101, false);

        // Assert
        assert!(!updated);
    }

    #[test]
    fn publisher_disconnect_tells_the_runners_of_its_downstream_subscriptions_why() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(2, 100, track_key, None)
            .unwrap()
            .stop_receiver;

        // Act
        table.remove_session(1);

        // Assert
        assert_eq!(
            runner_stop_receiver.try_recv(),
            Ok(PublishDoneReason::publisher_session_closed())
        );
    }

    #[test]
    fn publish_done_on_a_published_track_unregisters_its_publish_handler() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Publish);
        table.register_publish(
            PUBLISHER_SESSION,
            Arc::new(StubPublishHandler {
                track_namespace: track_key.track_namespace.clone(),
                track_namespace_tuple: vec![track_key.track_namespace.clone()],
                track_name: track_key.track_name.clone(),
                track_alias: 10,
            }),
        );

        // Act
        table.end_upstream_subscription(
            PUBLISHER_SESSION,
            UPSTREAM_REQUEST_ID,
            PublishDoneReason::publisher_session_closed(),
        );

        // Assert
        assert!(
            table
                .find_upstream_publishers(&track_key.track_namespace, &track_key.track_name)
                .is_empty()
        );
    }

    #[test]
    fn subscriber_disconnect_after_malformed_cleanup_stops_its_runner() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(2, 100, track_key.clone(), None)
            .unwrap()
            .stop_receiver;
        table.remove_upstream_subscription(&upstream_key()).unwrap();

        // Act
        table.remove_session(2);

        // Assert
        assert!(runner_stopped(&mut runner_stop_receiver));
        assert!(table.downstream_subscriptions.is_empty());
    }

    #[test]
    fn registration_for_a_removed_upstream_yields_no_runner() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        table.remove_session(1);

        // Act
        let runner_signals = table.register_downstream_subscription(2, 100, track_key, None);

        // Assert
        assert!(runner_signals.is_none());
        assert!(table.downstream_subscriptions.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registration_racing_publisher_removal_leaves_no_orphan() {
        for _ in 0..2000 {
            // Arrange
            let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
            let table = Arc::new(table);
            let barrier = Arc::new(tokio::sync::Barrier::new(2));

            // Act
            let register = tokio::spawn({
                let table = table.clone();
                let barrier = barrier.clone();
                async move {
                    barrier.wait().await;
                    table
                        .register_downstream_subscription(2, 100, track_key, None)
                        .is_some()
                }
            });
            let remove = tokio::spawn({
                let table = table.clone();
                async move {
                    barrier.wait().await;
                    table.remove_session(1)
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
            let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
            table
                .register_downstream_subscription(2, 100, track_key.clone(), None)
                .unwrap();
            let table = Arc::new(table);
            let barrier = Arc::new(tokio::sync::Barrier::new(2));

            // Act
            let register = tokio::spawn({
                let table = table.clone();
                let barrier = barrier.clone();
                let track_key = track_key.clone();
                async move {
                    barrier.wait().await;
                    table
                        .register_downstream_subscription(3, 200, track_key, None)
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
            let upstream_exists = table.upstream_tracks.contains_key(&track_key);
            assert_eq!(registered, upstream_exists);
            assert_eq!(
                removed.released_upstream_subscriptions.is_empty(),
                registered
            );
            assert_eq!(
                table.downstream_subscriptions.len(),
                usize::from(registered)
            );
        }
    }

    const SECOND_PUBLISHER_SESSION: SessionId = 3;
    const SECOND_UPSTREAM_REQUEST_ID: u64 = 44;

    fn add_second_publisher(
        table: &InMemoryLocalPubSubDirectory,
        track_key: &TrackKey,
        origin: UpstreamSubscriptionOrigin,
    ) {
        table.register_upstream_subscription(
            track_key.clone(),
            SECOND_PUBLISHER_SESSION,
            ActiveUpstreamSubscription {
                upstream_request_id: SECOND_UPSTREAM_REQUEST_ID,
                expires: None,
                content_exists: ContentExists::False,
                origin,
            },
        );
    }

    #[test]
    fn a_publisher_leaving_keeps_the_track_another_publisher_still_feeds() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        add_second_publisher(&table, &track_key, UpstreamSubscriptionOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(2, 100, track_key.clone(), None)
            .unwrap()
            .stop_receiver;

        // Act
        let removed = table.remove_session(PUBLISHER_SESSION);

        // Assert
        assert!(!runner_stopped(&mut runner_stop_receiver));
        assert_eq!(removed.upstream_track_keys, vec![track_key.clone()]);
        assert!(removed.downstream_subscriptions.is_empty());
        assert_eq!(
            table
                .get_upstream_track(&track_key)
                .map(|track| track.subscriptions.into_keys().collect::<Vec<_>>()),
            Some(vec![SECOND_PUBLISHER_SESSION])
        );
    }

    #[test]
    fn publish_done_from_one_publisher_keeps_the_track_another_publisher_still_feeds() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        add_second_publisher(&table, &track_key, UpstreamSubscriptionOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(2, 100, track_key.clone(), None)
            .unwrap()
            .stop_receiver;

        // Act
        let ended = table.end_upstream_subscription(
            SECOND_PUBLISHER_SESSION,
            SECOND_UPSTREAM_REQUEST_ID,
            PublishDoneReason::publisher_session_closed(),
        );

        // Assert
        assert_eq!(ended, Some(track_key));
        assert!(!runner_stopped(&mut runner_stop_receiver));
    }

    #[test]
    fn the_last_subscriber_releases_every_subscribe_initiated_upstream_subscription() {
        // Arrange
        let (table, track_key) = table_with_upstream(UpstreamSubscriptionOrigin::Subscribe);
        add_second_publisher(&table, &track_key, UpstreamSubscriptionOrigin::Subscribe);
        table.register_upstream_subscription(
            track_key.clone(),
            4,
            ActiveUpstreamSubscription {
                upstream_request_id: 45,
                expires: None,
                content_exists: ContentExists::False,
                origin: UpstreamSubscriptionOrigin::Publish,
            },
        );
        table
            .register_downstream_subscription(2, 100, track_key.clone(), None)
            .unwrap();

        // Act
        let removed = table.remove_downstream_subscription(2, 100).unwrap();

        // Assert
        assert_eq!(
            removed.released_upstream_subscriptions,
            vec![
                ReleasedUpstreamSubscription {
                    publisher_session_id: PUBLISHER_SESSION,
                    upstream_request_id: UPSTREAM_REQUEST_ID,
                },
                ReleasedUpstreamSubscription {
                    publisher_session_id: SECOND_PUBLISHER_SESSION,
                    upstream_request_id: SECOND_UPSTREAM_REQUEST_ID,
                },
            ]
        );
        assert_eq!(
            table
                .get_upstream_track(&track_key)
                .map(|track| track.subscriptions.into_keys().collect::<Vec<_>>()),
            Some(vec![4])
        );
    }

    #[test]
    fn finds_the_upstream_track_separately_from_publishers() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        let track_key = TrackKey::new("room/member", "video");
        table.register_upstream_subscription(
            track_key.clone(),
            1,
            ActiveUpstreamSubscription {
                upstream_request_id: 10,
                expires: Some(30),
                content_exists: ContentExists::False,
                origin: UpstreamSubscriptionOrigin::Subscribe,
            },
        );

        // Act
        let upstream_track = table.get_upstream_track(&track_key);
        let publisher_subscriptions = table.find_upstream_publishers("room/member", "video");

        // Assert
        assert_eq!(
            upstream_track.map(|track| track.subscriptions.into_keys().collect::<Vec<_>>()),
            Some(vec![1])
        );
        assert_eq!(
            publisher_subscriptions,
            vec![UpstreamSubscriptionKey {
                publisher_session_id: 1,
                track_namespace: "room/member".to_string(),
                track_name: "video".to_string(),
            }]
        );
    }
}

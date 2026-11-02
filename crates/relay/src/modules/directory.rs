pub(crate) mod track;

use std::{
    collections::{BTreeSet, HashSet, btree_map},
    sync::{Arc, OnceLock, PoisonError, RwLock},
};

use dashmap::{DashMap, DashSet, Entry};

use crate::modules::{
    directory::track::{
        Consumer, CreationFailure, CreationState, DownstreamSubscription, MatchingPublication,
        Producer, ProducerOrigin, ProducerState, ReleasedUpstreamSubscription,
        RemovedDownstreamSubscription, RemovedSessionSubscriptions, Track, TrackState,
    },
    domain::{
        delivery_control::{DeliveryControl, UpdateOutcome},
        delivery_stats::DeliveryCounters,
        publish_done_reason::PublishDoneReason,
        runner_signals::DownstreamRunnerSignals,
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::{TrackKey, TrackNamespace, TrackNamespacePrefix},
    },
    session::handler::publish::PublishHandler,
};
use moqt::fetch::JoinPoint;
use tokio::sync::watch;

/// What a SUBSCRIBE finds for its track. Each holds a `TrackReservation`,
/// so the track wants its producers for the caller already.
pub(crate) enum TrackLookup {
    /// Producers feed the track.
    Ready(TrackState, TrackReservation),
    /// Another SUBSCRIBE is creating the track; wait on its creation.
    Creating(watch::Receiver<CreationState>, TrackReservation),
    /// The caller is to create the track: resolve its publishers and send
    /// them SUBSCRIBE, or abandon the creation when there is none.
    Started(watch::Receiver<CreationState>, TrackReservation),
}

/// A forwarded PUBLISH between its send and its answer; dropping it lets the
/// track forward another PUBLISH to the session.
pub(crate) struct ForwardedPublish {
    directory: Arc<InMemoryLocalPubSubDirectory>,
    track_key: TrackKey,
    session_id: SessionId,
}

impl Drop for ForwardedPublish {
    fn drop(&mut self) {
        if let Some(mut track) = self.directory.upstream_tracks.get_mut(&self.track_key) {
            track.end_forwarded_publish(self.session_id);
        }
    }
}

/// Keeps a track wanting its producers for a subscriber that found it and is
/// about to register its consumer. Released on drop.
pub(crate) struct TrackReservation {
    directory: Arc<InMemoryLocalPubSubDirectory>,
    track_key: TrackKey,
    peer: SessionPeer,
}

impl Drop for TrackReservation {
    fn drop(&mut self) {
        if let Some(mut track) = self.directory.upstream_tracks.get_mut(&self.track_key) {
            track.release_reservation(self.peer);
        }
    }
}

pub(crate) struct DownstreamSubscriptionState {
    pub(crate) subscriber_session_id: SessionId,
    pub(crate) request_id: u64,
    pub(crate) track_key: TrackKey,
    pub(crate) delivery: DeliveryCounters,
}

pub(crate) struct RegisteredPublish {
    publisher_session_id: SessionId,
    publisher_peer: SessionPeer,
    handler: Arc<dyn PublishHandler>,
}

// Client subscriptions own the Redis route for their prefix, so the
// directory tracks the peer to detect when the last client leaves.
type PeersByNamespace = DashMap<String, DashMap<SessionId, SessionPeer>>;

/// Whether one of two `/`-joined namespace prefixes is the other's leading
/// fields: `a/b` overlaps `a` and `a/b/c`, not `a/bc`.
fn namespace_prefixes_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (namespace_fields(a), namespace_fields(b));
    let shared = a.len().min(b.len());
    a[..shared] == b[..shared]
}

fn namespace_fields(prefix: &str) -> Vec<&str> {
    prefix
        .split('/')
        .filter(|field| !field.is_empty())
        .collect()
}

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
    pub(crate) published_handlers: RwLock<Vec<RegisteredPublish>>,
    pub(crate) upstream_tracks: DashMap<TrackKey, Track>,
    /// The track of each consumer, keyed like `Track::consumers`. A track
    /// entry is locked before this index.
    consumer_tracks: DashMap<(SessionId, u64), TrackKey>,
    /// Unix milliseconds of the start of the drain.
    draining_since_ms: OnceLock<u64>,
}

impl InMemoryLocalPubSubDirectory {
    pub(crate) fn new() -> Self {
        Self {
            publisher_namespaces: DashMap::new(),
            subscriber_namespaces: DashMap::new(),
            published_handlers: RwLock::new(Vec::new()),
            upstream_tracks: DashMap::new(),
            consumer_tracks: DashMap::new(),
            draining_since_ms: OnceLock::new(),
        }
    }

    /// draft-22 §6.6.1: a draining relay unsubscribes from its publishers once
    /// the downstream subscribers it sent GOAWAY to have left, which tells a
    /// publisher publishing on both relays that it can stop on this one
    /// (draft-22 §7.6.1).
    pub(crate) fn begin_draining(&self, now_ms: u64) {
        let _ = self.draining_since_ms.set(now_ms);
    }

    pub(crate) fn draining_since_ms(&self) -> Option<u64> {
        self.draining_since_ms.get().copied()
    }

    pub(crate) fn release_unwatched_published_subscriptions(
        &self,
        publisher_session_id: SessionId,
    ) -> Vec<(TrackKey, ReleasedUpstreamSubscription)> {
        let mut released = Vec::new();
        for track_key in self.tracks_fed_by(publisher_session_id) {
            let Entry::Occupied(mut entry) = self.upstream_tracks.entry(track_key.clone()) else {
                continue;
            };
            let track = entry.get_mut();
            if track.downstream_remains() {
                continue;
            }
            let btree_map::Entry::Occupied(producer) = track.producers.entry(publisher_session_id)
            else {
                continue;
            };
            if producer.get().state.origin != ProducerOrigin::Publish {
                continue;
            }
            let producer = producer.remove();
            if track.producers.is_empty() {
                entry.remove();
            } else {
                drop(entry);
            }
            self.forget_published_handler(publisher_session_id, &track_key);
            released.push((
                track_key,
                ReleasedUpstreamSubscription {
                    publisher_session_id,
                    subscription: producer.state.subscription,
                },
            ));
        }
        released
    }

    fn forget_published_handler(&self, publisher_session_id: SessionId, track_key: &TrackKey) {
        self.published_handlers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|published| {
                published.publisher_session_id != publisher_session_id
                    || published.handler.track_namespace() != track_key.track_namespace
                    || published.handler.track_name() != track_key.track_name
            });
    }

    #[tracing::instrument(
        level = "debug",
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
            .retain(|published| published.publisher_session_id != session_id);

        let downstream_keys: Vec<_> = self
            .consumer_tracks
            .iter()
            .filter_map(|entry| (entry.key().0 == session_id).then_some(*entry.key()))
            .collect();
        for key in downstream_keys {
            if let Some(subscription) = self.remove_downstream_subscription(key.0, key.1) {
                removed.downstream_subscriptions.push(subscription);
            }
        }

        for track_key in self.tracks_fed_by(session_id) {
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
        level = "debug",
        name = "relay.local_pub_sub_directory.register_publish_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace = %track_namespace, session_peer = peer.kind())
    )]
    pub(crate) fn register_publish_namespace(
        &self,
        session_id: SessionId,
        track_namespace: String,
        peer: SessionPeer,
    ) {
        self.publisher_namespaces
            .entry(track_namespace)
            .or_default()
            .insert(session_id, peer);
    }

    /// Returns true when no client publisher remains for the namespace,
    /// i.e. the caller may clean up the Redis route.
    #[tracing::instrument(
        level = "debug",
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

    pub(crate) fn has_namespace_publishers(&self, track_namespace: &str) -> bool {
        self.publisher_namespaces.contains_key(track_namespace)
    }

    /// Drops relay-origin publisher namespaces under the prefix once no
    /// client subscriber covers them anymore. Relay-learned namespaces are
    /// only withdrawn by a best-effort PUBLISH_NAMESPACE_DONE, which can be
    /// missed when the remote publisher and the local subscriber leave at the
    /// same time; purging here lets the next subscriber re-learn them from
    /// the route registry instead of a stale local copy.
    #[tracing::instrument(
        level = "debug",
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
        level = "debug",
        name = "relay.local_pub_sub_directory.register_subscribe_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix, session_peer = peer.kind())
    )]
    pub(crate) fn register_subscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: String,
        peer: SessionPeer,
    ) -> bool {
        let sessions = self
            .subscriber_namespaces
            .entry(track_namespace_prefix.clone())
            .or_insert_with(|| {
                tracing::debug!(
                    session_id = %session_id,
                    track_namespace_prefix = %track_namespace_prefix,
                    "namespace prefix subscribed"
                );
                DashMap::new()
            });
        let had_client = has_client(&sessions);
        sessions.insert(session_id, peer);
        peer == SessionPeer::Client && !had_client
    }

    /// Returns true when no client subscriber remains for the prefix,
    /// i.e. the caller may clean up the Redis route.
    #[tracing::instrument(
        level = "debug",
        name = "relay.local_pub_sub_directory.unregister_subscribe_namespace",
        skip_all,
        fields(session_id = %session_id, track_namespace_prefix = %track_namespace_prefix)
    )]
    /// Whether `session_id` has an active SUBSCRIBE_NAMESPACE whose prefix is a
    /// prefix of, equal to or an extension of `track_namespace_prefix`, compared
    /// field by field (draft-14 §9.28).
    pub(crate) fn has_overlapping_subscribe_namespace(
        &self,
        session_id: SessionId,
        track_namespace_prefix: &str,
    ) -> bool {
        self.subscriber_namespaces.iter().any(|entry| {
            entry.value().contains_key(&session_id)
                && namespace_prefixes_overlap(entry.key(), track_namespace_prefix)
        })
    }

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
        level = "debug",
        name = "relay.local_pub_sub_directory.register_publish",
        skip_all,
        fields(session_id = %session_id)
    )]
    pub(crate) fn register_publish(
        &self,
        session_id: SessionId,
        peer: SessionPeer,
        handler: Arc<dyn PublishHandler>,
    ) {
        self.published_handlers
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .push(RegisteredPublish {
                publisher_session_id: session_id,
                publisher_peer: peer,
                handler,
            });
    }

    #[tracing::instrument(
        level = "debug",
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
        level = "debug",
        name = "relay.local_pub_sub_directory.matching_publications",
        skip_all,
        fields(track_namespace_prefix = %track_namespace_prefix)
    )]
    pub(crate) fn matching_publications(
        &self,
        track_namespace_prefix: &str,
    ) -> HashSet<MatchingPublication> {
        let mut matching = HashSet::new();
        for entry in self.publisher_namespaces.iter() {
            if entry.key().starts_with(track_namespace_prefix) {
                matching.insert(MatchingPublication::Namespace(entry.key().clone()));
            }
        }

        for RegisteredPublish { handler, .. } in self
            .published_handlers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            if handler
                .track_namespace()
                .starts_with(track_namespace_prefix)
            {
                matching.insert(MatchingPublication::Track(TrackKey::new(
                    handler.track_namespace(),
                    handler.track_name(),
                )));
            }
        }
        matching
    }

    #[tracing::instrument(
        level = "debug",
        name = "relay.local_pub_sub_directory.find_upstream_publishers",
        skip_all,
        fields(track_namespace = %track_namespace, track_name = %track_name)
    )]
    /// Local client publishers only, oldest first: publishing relays are
    /// found in the route registry, which lists each of them once.
    pub(crate) fn find_upstream_publishers(
        &self,
        track_namespace: &str,
        track_name: &str,
    ) -> Vec<SessionId> {
        let mut publishers = BTreeSet::new();
        if let Some(namespace_publishers) = self.publisher_namespaces.get(track_namespace) {
            publishers.extend(
                namespace_publishers
                    .iter()
                    .filter(|session| *session.value() == SessionPeer::Client)
                    .map(|session| *session.key()),
            );
        }
        publishers.extend(
            self.published_handlers
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter(|published| {
                    published.publisher_peer == SessionPeer::Client
                        && published.handler.track_namespace() == track_namespace
                        && published.handler.track_name() == track_name
                })
                .map(|published| published.publisher_session_id),
        );
        publishers.into_iter().collect()
    }

    pub(crate) fn tracks_wanting_publisher(
        &self,
        track_namespace: &str,
        publisher_peer: SessionPeer,
        publisher_session_id: Option<SessionId>,
    ) -> Vec<TrackKey> {
        self.upstream_tracks
            .iter()
            .filter(|entry| {
                let track = entry.value();
                entry.key().track_namespace == track_namespace
                    && track.wants(publisher_peer)
                    && publisher_session_id
                        .is_none_or(|publisher| !track.producers.contains_key(&publisher))
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    pub(crate) fn watched_namespaces(&self) -> HashSet<TrackNamespace> {
        self.upstream_tracks
            .iter()
            .filter(|entry| entry.value().has_client_consumer())
            .map(|entry| entry.key().track_namespace.clone())
            .collect()
    }

    /// The tracks the session holds a producer of, whatever its origin
    /// (PUBLISH or SUBSCRIBE).
    pub(crate) fn tracks_fed_by(&self, publisher_session_id: SessionId) -> Vec<TrackKey> {
        self.upstream_tracks
            .iter()
            .filter(|entry| entry.value().producers.contains_key(&publisher_session_id))
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// One entry per publisher feeding each track.
    pub(crate) fn active_upstream_tracks(&self) -> Vec<(SessionId, TrackKey)> {
        self.upstream_tracks
            .iter()
            .flat_map(|entry| {
                let track_key = entry.key().clone();
                entry
                    .value()
                    .producers
                    .keys()
                    .map(|publisher_session_id| (*publisher_session_id, track_key.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(crate) fn downstream_subscription_states(&self) -> Vec<DownstreamSubscriptionState> {
        self.upstream_tracks
            .iter()
            .flat_map(|entry| {
                entry
                    .value()
                    .consumers
                    .iter()
                    .map(|((subscriber_session_id, request_id), consumer)| {
                        DownstreamSubscriptionState {
                            subscriber_session_id: *subscriber_session_id,
                            request_id: *request_id,
                            track_key: entry.key().clone(),
                            delivery: consumer.delivery_counters(),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// While true, this relay owns the track's route.
    pub(crate) fn has_client_published_track(&self, track_key: &TrackKey) -> bool {
        self.published_handlers
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|published| {
                published.publisher_peer == SessionPeer::Client
                    && published.handler.track_namespace() == track_key.track_namespace
                    && published.handler.track_name() == track_key.track_name
            })
    }

    /// Some publisher still serves the track: an upstream subscription, a
    /// client publishing its namespace, or a pending PUBLISH of it.
    pub(crate) fn has_track_publisher(&self, track_key: &TrackKey) -> bool {
        self.upstream_tracks.contains_key(track_key)
            || !self
                .find_upstream_publishers(&track_key.track_namespace, &track_key.track_name)
                .is_empty()
    }

    /// `None` for a track no producer feeds yet (see `TrackLookup`).
    pub(crate) fn track_state(&self, track_key: &TrackKey) -> Option<TrackState> {
        self.upstream_tracks
            .get(track_key)
            .filter(|entry| !entry.producers.is_empty())
            .map(|entry| entry.value().state())
    }

    /// Finds the track for a SUBSCRIBE from a `requester` peer, or starts its
    /// creation: the entry is inserted `Creating` so that the SUBSCRIBEs
    /// that follow wait for this one instead of subscribing upstream again.
    pub(crate) fn track_or_begin_creation(
        self: &Arc<Self>,
        track_key: &TrackKey,
        requester: SessionPeer,
    ) -> TrackLookup {
        let reservation = TrackReservation {
            directory: self.clone(),
            track_key: track_key.clone(),
            peer: requester,
        };
        match self.upstream_tracks.entry(track_key.clone()) {
            Entry::Occupied(mut entry) => {
                let track = entry.get_mut();
                track.reserve(requester);
                if track.producers.is_empty() {
                    TrackLookup::Creating(track.creation(), reservation)
                } else {
                    TrackLookup::Ready(track.state(), reservation)
                }
            }
            Entry::Vacant(entry) => {
                let mut track = Track::new(CreationState::Creating);
                track.reserve(requester);
                let creation = track.creation();
                entry.insert(track);
                TrackLookup::Started(creation, reservation)
            }
        }
    }

    pub(crate) fn add_pending_producers(&self, track_key: &TrackKey, count: usize) {
        if let Some(mut track) = self.upstream_tracks.get_mut(track_key) {
            track.add_pending_producers(count);
        }
    }

    /// An upstream SUBSCRIBE for the track ended without a producer. When it
    /// was the last one of a track still being created, the creation fails
    /// and the entry goes.
    pub(crate) fn pending_producer_ended(
        &self,
        track_key: &TrackKey,
        failure: Option<CreationFailure>,
    ) {
        self.upstream_tracks
            .remove_if_mut(track_key, |_, track| track.pending_producer_ended(failure));
    }

    fn with_consumer<T>(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        read: impl FnOnce(&Consumer) -> T,
    ) -> Option<T> {
        let key = (downstream_session_id, downstream_subscribe_id);
        let track_key = self.consumer_tracks.get(&key)?.clone();
        self.upstream_tracks
            .get(&track_key)?
            .consumers
            .get(&key)
            .map(read)
    }

    pub(crate) fn get_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<DownstreamSubscription> {
        self.with_consumer(downstream_session_id, downstream_subscribe_id, |consumer| {
            consumer.subscription.clone()
        })
    }

    #[cfg(test)]
    pub(crate) fn downstream_delivery_control(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<DeliveryControl> {
        self.with_consumer(
            downstream_session_id,
            downstream_subscribe_id,
            Consumer::delivery_control,
        )
    }

    /// Marks a PUBLISH for the track as on its way to `session_id`, so no
    /// second one is sent meanwhile. `None` when the track is gone, the
    /// session already receives it, or a PUBLISH to it is already on its way.
    pub(crate) fn begin_forwarded_publish(
        self: &Arc<Self>,
        track_key: &TrackKey,
        session_id: SessionId,
    ) -> Option<ForwardedPublish> {
        self.upstream_tracks
            .get_mut(track_key)?
            .begin_forwarded_publish(session_id)
            .then(|| ForwardedPublish {
                directory: self.clone(),
                track_key: track_key.clone(),
                session_id,
            })
    }

    /// Returns the producer from the same publisher that this one replaces;
    /// its ingest stops when the caller drops it.
    pub(crate) fn register_upstream_subscription(
        &self,
        track_key: TrackKey,
        publisher_session_id: SessionId,
        producer: Producer,
    ) -> Option<Producer> {
        let mut track = self
            .upstream_tracks
            .entry(track_key.clone())
            .or_insert_with(|| Track::new(CreationState::Created));
        let replaced = track.producers.insert(publisher_session_id, producer);
        track.mark_created();
        drop(track);
        if replaced
            .as_ref()
            .is_some_and(|replaced| replaced.state.origin == ProducerOrigin::Publish)
        {
            self.forget_published_handler(publisher_session_id, &track_key);
        }
        replaced
    }

    pub(crate) fn published_upstream_subscription(
        &self,
        track_key: &TrackKey,
        publisher_session_id: SessionId,
    ) -> Option<ProducerState> {
        self.upstream_tracks
            .get(track_key)?
            .producers
            .get(&publisher_session_id)
            .filter(|producer| producer.state.origin == ProducerOrigin::Publish)
            .map(|producer| producer.state.clone())
    }

    /// Whether `add_upstream_subscription_to_track` would take a producer
    /// from this publisher now.
    pub(crate) fn track_takes_publisher(
        &self,
        track_key: &TrackKey,
        publisher_session_id: SessionId,
        publisher_peer: SessionPeer,
    ) -> bool {
        self.upstream_tracks
            .get(track_key)
            .is_some_and(|track| track.takes(publisher_session_id, publisher_peer))
    }

    /// Returns false, adding nothing, when the track is gone, the publisher
    /// already feeds it, or the track does not want it. The first producer
    /// of a track being created completes the creation.
    pub(crate) fn add_upstream_subscription_to_track(
        &self,
        track_key: &TrackKey,
        publisher_session_id: SessionId,
        producer: Producer,
    ) -> bool {
        let Some(mut track) = self.upstream_tracks.get_mut(track_key) else {
            return false;
        };
        if !track.takes(publisher_session_id, producer.state.peer) {
            return false;
        }
        track.producers.insert(publisher_session_id, producer);
        track.mark_created();
        true
    }

    #[cfg(test)]
    pub(crate) fn has_upstream_subscription(
        &self,
        track_key: &TrackKey,
        publisher_session_id: SessionId,
        upstream_request_id: u64,
    ) -> bool {
        self.upstream_tracks.get(track_key).is_some_and(|track| {
            track
                .producers
                .get(&publisher_session_id)
                .is_some_and(|producer| {
                    producer.state.subscription.request_id() == upstream_request_id
                })
        })
    }

    /// Leaves the track's downstream registrations to their subscribers: a
    /// malformed track ends their runners on its own.
    pub(crate) fn remove_upstream_track(&self, track_key: &TrackKey) -> Vec<(SessionId, Producer)> {
        self.upstream_tracks
            .remove(track_key)
            .map(|(_, track)| track.producers.into_iter().collect())
            .unwrap_or_default()
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
                    .producers
                    .get(&publisher_session_id)
                    .is_some_and(|producer| {
                        producer.state.subscription.request_id() == upstream_request_id
                    })
            })
            .map(|entry| entry.key().clone())?;
        let (ended, _) = self.end_publisher_subscription(&track_key, publisher_session_id, end)?;
        if ended.state.origin == ProducerOrigin::Publish {
            self.forget_published_handler(publisher_session_id, &track_key);
        }
        Some(track_key)
    }

    /// The downstream subscriptions are swept while the track entry is still
    /// held, so a track recreated under the same key keeps its own.
    fn end_publisher_subscription(
        &self,
        track_key: &TrackKey,
        publisher_session_id: SessionId,
        end: PublishDoneReason,
    ) -> Option<(Producer, Vec<RemovedDownstreamSubscription>)> {
        let Entry::Occupied(mut track) = self.upstream_tracks.entry(track_key.clone()) else {
            return None;
        };
        let ended = track.get_mut().producers.remove(&publisher_session_id)?;
        if !track.get().producers.is_empty() {
            return Some((ended, Vec::new()));
        }
        let removed = self.end_downstream_subscriptions(track.get_mut(), track_key, end);
        track.remove();
        Some((ended, removed))
    }

    /// Returns `None` when the upstream track is gone. The returned
    /// stop receiver resolves once the registration is removed, however that happens; the
    /// subscription's egress runner lives exactly until then.
    pub(crate) fn register_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        subscriber_peer: SessionPeer,
        track_key: TrackKey,
        join_point: JoinPoint,
        delivery_control: DeliveryControl,
    ) -> Option<DownstreamRunnerSignals> {
        // The track entry stays locked until the consumer is inserted: a
        // concurrent removal of the track either finds it or makes this
        // registration fail.
        let mut track = self.upstream_tracks.get_mut(&track_key)?;
        let (consumer, signals) = Consumer::new(
            DownstreamSubscription {
                track_key: track_key.clone(),
                join_point,
            },
            subscriber_peer,
            delivery_control,
        );
        let key = (downstream_session_id, downstream_subscribe_id);
        track.consumers.insert(key, consumer);
        self.consumer_tracks.insert(key, track_key);
        Some(signals)
    }

    /// Applies a SUBSCRIBE_UPDATE to the registration's delivery control (see
    /// `DeliveryControl::apply_update`). Returns `None` when no such downstream
    /// subscription is registered.
    pub(crate) fn update_downstream_delivery(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
        update: DeliveryControl,
    ) -> Option<UpdateOutcome> {
        self.with_consumer(downstream_session_id, downstream_subscribe_id, |consumer| {
            consumer.update_delivery(update)
        })
    }

    /// The last downstream subscriber leaving releases the track's
    /// SUBSCRIBE-initiated upstream subscriptions, and the last client
    /// subscriber leaving releases those from relays: a relay's upstream
    /// subscription only serves this relay's clients, so two relays feeding
    /// each other never keep a track alive between them. PUBLISH-initiated
    /// subscriptions stay until their publisher ends them, except while the
    /// relay drains, when the last downstream subscriber releases them too.
    /// Relay subscribers left on a track whose every upstream subscription
    /// was released end with TRACK_ENDED.
    pub(crate) fn remove_downstream_subscription(
        &self,
        downstream_session_id: SessionId,
        downstream_subscribe_id: u64,
    ) -> Option<RemovedDownstreamSubscription> {
        let key = (downstream_session_id, downstream_subscribe_id);
        let track_key = self.consumer_tracks.get(&key)?.clone();
        let Entry::Occupied(mut entry) = self.upstream_tracks.entry(track_key.clone()) else {
            self.consumer_tracks.remove(&key);
            return None;
        };
        let track = entry.get_mut();
        // Dropping the consumer stops its runner without PUBLISH_DONE: the
        // subscriber ended the subscription (draft-14 §5.1).
        let removed_consumer = track.consumers.remove(&key);
        self.consumer_tracks.remove(&key);
        removed_consumer?;
        let downstream_remains = track.downstream_remains();
        let client_remains = track.client_remains();
        let draining = self.draining_since_ms.get().is_some();
        let mut released_upstream_subscriptions = Vec::new();
        let mut released_published = Vec::new();
        if !client_remains {
            track.producers.retain(|publisher_session_id, producer| {
                let state = &producer.state;
                let released = match state.origin {
                    ProducerOrigin::Subscribe => {
                        !downstream_remains || state.peer == SessionPeer::Relay
                    }
                    ProducerOrigin::Publish => draining && !downstream_remains,
                };
                if released && state.origin == ProducerOrigin::Publish {
                    released_published.push(*publisher_session_id);
                }
                if released {
                    released_upstream_subscriptions.push(ReleasedUpstreamSubscription {
                        publisher_session_id: *publisher_session_id,
                        subscription: state.subscription.clone(),
                    });
                }
                !released
            });
        }
        if track.producers.is_empty() {
            self.end_downstream_subscriptions(
                track,
                &track_key,
                PublishDoneReason::publisher_session_closed(),
            );
            entry.remove();
        } else {
            drop(entry);
        }
        for publisher_session_id in released_published {
            self.forget_published_handler(publisher_session_id, &track_key);
        }
        Some(RemovedDownstreamSubscription {
            track_key,
            released_upstream_subscriptions,
        })
    }

    /// Ends every consumer of `track`, whose last producer ended, with the
    /// PUBLISH_DONE reason `end`. The caller holds the track's entry.
    fn end_downstream_subscriptions(
        &self,
        track: &mut Track,
        track_key: &TrackKey,
        end: PublishDoneReason,
    ) -> Vec<RemovedDownstreamSubscription> {
        track
            .consumers
            .drain()
            .map(|(key, consumer)| {
                self.consumer_tracks.remove(&key);
                consumer.end(end.clone());
                RemovedDownstreamSubscription {
                    track_key: track_key.clone(),
                    released_upstream_subscriptions: Vec::new(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::test_support::directory_fixtures::delivery_control;
    use crate::modules::test_support::directory_fixtures::{
        PUBLISHER_SESSION, UPSTREAM_REQUEST_ID, active_upstream, producer, producer_with,
        table_with_upstream, track_key,
    };
    use crate::modules::test_support::mock_session::{MockPublishHandler, runner_stopped};
    use crate::modules::test_support::relay_harness::fixtures::subscription::upstream_subscription;

    #[tokio::test]
    async fn a_track_keeps_a_publisher_while_another_session_publishes_its_namespace() {
        // Arrange: session 1 serves the track, session 2 publishes the namespace
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        table.register_publish_namespace(2, "ns".to_string(), SessionPeer::Client);

        // Act
        table.remove_session(PUBLISHER_SESSION);

        // Assert
        assert!(table.has_track_publisher(&track_key));
    }

    #[tokio::test]
    async fn a_track_has_no_publisher_once_its_only_publisher_left() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);

        // Act
        table.remove_session(PUBLISHER_SESSION);

        // Assert
        assert!(!table.has_track_publisher(&track_key));
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
            SessionPeer::Client,
            Arc::new(MockPublishHandler::new("room/member", "video", 10)),
        );

        // Act: Remove all state associated with session 1.
        let removed = table.remove_session(1);

        // Assert: Remove only session 1 state while keeping other publishers in the same namespace.
        assert_eq!(
            table.find_upstream_publishers("room/member", "video"),
            vec![2]
        );

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
    fn a_subscribe_namespace_overlaps_the_session_s_own_prefixes_field_by_field() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_subscribe_namespace(1, "app/room".to_string(), SessionPeer::Client);

        // Act
        let overlaps = ["app/room", "app", "app/room/alice", "app/room1"]
            .map(|prefix| table.has_overlapping_subscribe_namespace(1, prefix));
        let other_session = table.has_overlapping_subscribe_namespace(2, "app/room");

        // Assert: `app/room1` differs in its second field; another session never overlaps
        assert_eq!(overlaps, [true, true, true, false]);
        assert!(!other_session);
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
        assert!(
            table
                .publisher_namespaces
                .get("room/member")
                .is_some_and(|publishers| publishers.contains_key(&2))
        );
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
        assert!(
            table
                .publisher_namespaces
                .get("room/member")
                .is_some_and(|publishers| publishers.contains_key(&2))
        );
    }

    #[test]
    fn allows_multiple_publishers_for_the_same_namespace_and_track() {
        // Arrange: Register multiple publishers for the same namespace and track.
        let table = InMemoryLocalPubSubDirectory::new();

        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        table.register_publish_namespace(2, "room/member".to_string(), SessionPeer::Client);
        table.register_publish(
            1,
            SessionPeer::Client,
            Arc::new(MockPublishHandler::new("room/member", "video", 10)),
        );
        table.register_publish(
            2,
            SessionPeer::Client,
            Arc::new(MockPublishHandler::new("room/member", "video", 20)),
        );

        // Act: Find upstream publishers available for subscribe.
        let upstream_publishers = table.find_upstream_publishers("room/member", "video");

        // Assert: Keep multiple publishers regardless of whether they came from namespace or track state.
        assert_eq!(upstream_publishers, vec![1, 2]);
    }

    #[test]
    fn a_track_stays_client_published_while_another_client_publishes_it() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let track_key = TrackKey::new("room/member", "video");
        for (session_id, peer) in [
            (1, SessionPeer::Client),
            (2, SessionPeer::Client),
            (3, SessionPeer::Relay),
        ] {
            table.register_publish(
                session_id,
                peer,
                Arc::new(MockPublishHandler::new("room/member", "video", 10)),
            );
        }

        // Act
        table.remove_session(1);
        let after_one_client_left = table.has_client_published_track(&track_key);
        table.remove_session(2);
        let after_both_clients_left = table.has_client_published_track(&track_key);

        // Assert: the relay's PUBLISH does not count
        assert!(after_one_client_left);
        assert!(!after_both_clients_left);
    }

    #[tokio::test]
    async fn register_downstream_subscription_stores_join_point() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let largest = moqt::object::Location {
            group_id: 5,
            object_id: 3,
        };

        // Act
        let runner_signals = table.register_downstream_subscription(
            2,
            100,
            SessionPeer::Client,
            track_key.clone(),
            JoinPoint::Largest(Some(largest)),
            delivery_control(true),
        );

        // Assert
        assert!(runner_signals.is_some());
        let sub = table.get_downstream_subscription(2, 100).unwrap();
        assert_eq!(sub.track_key, track_key);
        assert_eq!(sub.join_point, JoinPoint::Largest(Some(largest)));
    }

    #[tokio::test]
    async fn registered_subscription_starts_forwarding() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);

        // Act
        let signals = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key,
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();

        // Assert
        assert!(signals.delivery_control_receiver.borrow().forward);
    }

    #[tokio::test]
    async fn forward_update_reaches_the_registered_runner() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let signals = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key,
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();

        // Act
        let outcome = table.update_downstream_delivery(2, 100, delivery_control(false));

        // Assert
        assert_eq!(outcome, Some(UpdateOutcome::Applied));
        assert!(!signals.delivery_control_receiver.borrow().forward);
    }

    #[tokio::test]
    async fn downstream_subscription_states_report_delivery() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let signals = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();
        signals.delivery_stats.record_stream_reset();

        // Act
        let states = table.downstream_subscription_states();

        // Assert
        let [state] = &states[..] else {
            panic!("expected one registration, got {}", states.len());
        };
        assert_eq!((state.subscriber_session_id, state.request_id), (2, 100));
        assert_eq!(state.track_key, track_key);
        assert_eq!(state.delivery.streams_reset, 1);
    }

    #[tokio::test]
    async fn active_upstream_tracks_lists_each_publisher_with_its_track() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);

        // Act
        let tracks = table.active_upstream_tracks();

        // Assert
        assert_eq!(tracks, vec![(PUBLISHER_SESSION, track_key)]);
    }

    #[tokio::test]
    async fn tracks_fed_by_lists_only_the_session_s_tracks() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);

        // Act / Assert
        assert_eq!(table.tracks_fed_by(PUBLISHER_SESSION), vec![track_key]);
        assert!(table.tracks_fed_by(PUBLISHER_SESSION + 1).is_empty());
    }

    #[tokio::test]
    async fn forward_update_for_an_unknown_subscription_is_reported() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let _signals = table.register_downstream_subscription(
            2,
            100,
            SessionPeer::Client,
            track_key,
            JoinPoint::Largest(None),
            delivery_control(true),
        );

        // Act
        let outcome = table.update_downstream_delivery(2, 101, delivery_control(false));

        // Assert
        assert_eq!(outcome, None);
    }

    #[tokio::test]
    async fn publisher_disconnect_tells_the_runners_of_its_downstream_subscriptions_why() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key,
                JoinPoint::Largest(None),
                delivery_control(true),
            )
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

    #[tokio::test]
    async fn publish_done_on_a_published_track_unregisters_its_publish_handler() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Publish);
        table.register_publish(
            PUBLISHER_SESSION,
            SessionPeer::Client,
            Arc::new(MockPublishHandler::new(
                &track_key.track_namespace,
                &track_key.track_name,
                10,
            )),
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

    #[tokio::test]
    async fn subscriber_disconnect_after_malformed_cleanup_stops_its_runner() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap()
            .stop_receiver;
        assert!(!table.remove_upstream_track(&track_key).is_empty());

        // Act
        table.remove_session(2);

        // Assert
        assert!(runner_stopped(&mut runner_stop_receiver));
        assert!(table.downstream_subscription_states().is_empty());
    }

    #[tokio::test]
    async fn registration_for_a_removed_upstream_yields_no_runner() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        table.remove_session(1);

        // Act
        let runner_signals = table.register_downstream_subscription(
            2,
            100,
            SessionPeer::Client,
            track_key,
            JoinPoint::Largest(None),
            delivery_control(true),
        );

        // Assert
        assert!(runner_signals.is_none());
        assert!(table.downstream_subscription_states().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registration_racing_publisher_removal_leaves_no_orphan() {
        for _ in 0..2000 {
            // Arrange
            let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
            let table = Arc::new(table);
            let barrier = Arc::new(tokio::sync::Barrier::new(2));

            // Act
            let register = tokio::spawn({
                let table = table.clone();
                let barrier = barrier.clone();
                async move {
                    barrier.wait().await;
                    table
                        .register_downstream_subscription(
                            2,
                            100,
                            SessionPeer::Client,
                            track_key,
                            JoinPoint::Largest(None),
                            delivery_control(true),
                        )
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
            assert!(table.downstream_subscription_states().is_empty());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn registration_racing_last_subscriber_removal_leaves_no_orphan() {
        for _ in 0..20000 {
            // Arrange
            let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
            table
                .register_downstream_subscription(
                    2,
                    100,
                    SessionPeer::Client,
                    track_key.clone(),
                    JoinPoint::Largest(None),
                    delivery_control(true),
                )
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
                        .register_downstream_subscription(
                            3,
                            200,
                            SessionPeer::Client,
                            track_key,
                            JoinPoint::Largest(None),
                            delivery_control(true),
                        )
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
                table.downstream_subscription_states().len(),
                usize::from(registered)
            );
        }
    }

    fn released_request_ids(removed: &RemovedDownstreamSubscription) -> Vec<(SessionId, u64)> {
        removed
            .released_upstream_subscriptions
            .iter()
            .map(|released| {
                (
                    released.publisher_session_id,
                    released.subscription.request_id(),
                )
            })
            .collect()
    }

    const SECOND_PUBLISHER_SESSION: SessionId = 3;
    const SECOND_UPSTREAM_REQUEST_ID: u64 = 44;

    fn add_second_publisher(
        table: &InMemoryLocalPubSubDirectory,
        track_key: &TrackKey,
        origin: ProducerOrigin,
    ) {
        table.register_upstream_subscription(
            track_key.clone(),
            SECOND_PUBLISHER_SESSION,
            producer_with(ProducerState {
                subscription: upstream_subscription(SECOND_UPSTREAM_REQUEST_ID),
                ..active_upstream(origin)
            }),
        );
    }

    #[tokio::test]
    async fn a_publisher_leaving_keeps_the_track_another_publisher_still_feeds() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        add_second_publisher(&table, &track_key, ProducerOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
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
                .track_state(&track_key)
                .map(|track| track.producers.into_keys().collect::<Vec<_>>()),
            Some(vec![SECOND_PUBLISHER_SESSION])
        );
    }

    #[tokio::test]
    async fn publish_done_from_one_publisher_keeps_the_track_another_publisher_still_feeds() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        add_second_publisher(&table, &track_key, ProducerOrigin::Subscribe);
        let mut runner_stop_receiver = table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
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

    #[tokio::test]
    async fn the_last_subscriber_releases_every_subscribe_initiated_upstream_subscription() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        add_second_publisher(&table, &track_key, ProducerOrigin::Subscribe);
        table.register_upstream_subscription(
            track_key.clone(),
            4,
            producer_with(ProducerState {
                subscription: upstream_subscription(45),
                ..active_upstream(ProducerOrigin::Publish)
            }),
        );
        table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();

        // Act
        let removed = table.remove_downstream_subscription(2, 100).unwrap();

        // Assert
        assert_eq!(
            released_request_ids(&removed),
            vec![
                (PUBLISHER_SESSION, UPSTREAM_REQUEST_ID),
                (SECOND_PUBLISHER_SESSION, SECOND_UPSTREAM_REQUEST_ID),
            ]
        );
        assert_eq!(
            table
                .track_state(&track_key)
                .map(|track| track.producers.into_keys().collect::<Vec<_>>()),
            Some(vec![4])
        );
    }

    const RELAY_PUBLISHER_SESSION: SessionId = 5;
    const RELAY_SUBSCRIBER_SESSION: SessionId = 6;

    fn add_relay_publisher(table: &InMemoryLocalPubSubDirectory, track_key: &TrackKey) {
        table.register_upstream_subscription(
            track_key.clone(),
            RELAY_PUBLISHER_SESSION,
            producer_with(ProducerState {
                subscription: upstream_subscription(46),
                peer: SessionPeer::Relay,
                ..active_upstream(ProducerOrigin::Subscribe)
            }),
        );
    }

    #[tokio::test]
    async fn the_last_client_subscriber_releases_the_upstream_subscriptions_from_relays() {
        // Arrange
        let (table, track_key) = table_with_upstream(ProducerOrigin::Subscribe);
        add_relay_publisher(&table, &track_key);
        table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();
        let mut relay_runner_stop_receiver = table
            .register_downstream_subscription(
                RELAY_SUBSCRIBER_SESSION,
                200,
                SessionPeer::Relay,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap()
            .stop_receiver;

        // Act
        let removed = table.remove_downstream_subscription(2, 100).unwrap();

        // Assert: the client publisher keeps serving the relay subscriber
        assert_eq!(
            released_request_ids(&removed),
            vec![(RELAY_PUBLISHER_SESSION, 46)]
        );
        assert!(!runner_stopped(&mut relay_runner_stop_receiver));
        assert_eq!(
            table
                .track_state(&track_key)
                .map(|track| track.producers.into_keys().collect::<Vec<_>>()),
            Some(vec![PUBLISHER_SESSION])
        );
    }

    #[tokio::test]
    async fn a_relay_subscriber_left_on_a_track_fed_only_by_relays_ends_with_it() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        let track_key = TrackKey::new("ns", "track");
        add_relay_publisher(&table, &track_key);
        table
            .register_downstream_subscription(
                2,
                100,
                SessionPeer::Client,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap();
        let mut relay_runner_stop_receiver = table
            .register_downstream_subscription(
                RELAY_SUBSCRIBER_SESSION,
                200,
                SessionPeer::Relay,
                track_key.clone(),
                JoinPoint::Largest(None),
                delivery_control(true),
            )
            .unwrap()
            .stop_receiver;

        // Act
        table.remove_downstream_subscription(2, 100);

        // Assert
        assert_eq!(
            relay_runner_stop_receiver.try_recv(),
            Ok(PublishDoneReason::publisher_session_closed())
        );
        assert!(table.upstream_tracks.is_empty());
        assert!(table.downstream_subscription_states().is_empty());
    }

    #[tokio::test]
    async fn a_relay_publisher_is_wanted_only_on_tracks_a_client_watches() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        for (track_name, subscriber_peer) in [
            ("client_watched", SessionPeer::Client),
            ("relay_watched", SessionPeer::Relay),
        ] {
            let track_key = TrackKey::new("ns", track_name);
            table.register_upstream_subscription(
                track_key.clone(),
                PUBLISHER_SESSION,
                producer(ProducerOrigin::Subscribe),
            );
            table
                .register_downstream_subscription(
                    2,
                    100 + track_name.len() as u64,
                    subscriber_peer,
                    track_key,
                    JoinPoint::Largest(None),
                    delivery_control(true),
                )
                .unwrap();
        }

        // Act
        let relay_tracks =
            table.tracks_wanting_publisher("ns", SessionPeer::Relay, Some(RELAY_PUBLISHER_SESSION));
        let mut client_tracks = table.tracks_wanting_publisher(
            "ns",
            SessionPeer::Client,
            Some(RELAY_PUBLISHER_SESSION),
        );
        client_tracks.sort_by(|a, b| a.track_name.cmp(&b.track_name));

        // Assert
        assert_eq!(relay_tracks, vec![TrackKey::new("ns", "client_watched")]);
        assert_eq!(
            client_tracks,
            vec![
                TrackKey::new("ns", "client_watched"),
                TrackKey::new("ns", "relay_watched")
            ]
        );
        assert_eq!(
            table.watched_namespaces(),
            HashSet::from(["ns".to_string()])
        );
    }

    #[tokio::test]
    async fn finds_the_upstream_track_separately_from_publishers() {
        // Arrange
        let table = InMemoryLocalPubSubDirectory::new();
        table.register_publish_namespace(1, "room/member".to_string(), SessionPeer::Client);
        let track_key = TrackKey::new("room/member", "video");
        table.register_upstream_subscription(
            track_key.clone(),
            1,
            producer_with(ProducerState {
                subscription: upstream_subscription(10),
                ..active_upstream(ProducerOrigin::Subscribe)
            }),
        );

        // Act
        let upstream_track = table.track_state(&track_key);
        let publisher_subscriptions = table.find_upstream_publishers("room/member", "video");

        // Assert
        assert_eq!(
            upstream_track.map(|track| track.producers.into_keys().collect::<Vec<_>>()),
            Some(vec![1])
        );
        assert_eq!(publisher_subscriptions, vec![1]);
    }

    fn creation_failure(reason: &str) -> CreationFailure {
        CreationFailure {
            code: moqt::RequestErrorCode::InternalError,
            reason: reason.to_string(),
        }
    }

    #[tokio::test]
    async fn the_first_subscribe_starts_the_creation_and_the_next_waits_for_it() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        let TrackLookup::Started(creation, _reservation) =
            table.track_or_begin_creation(&track_key(), SessionPeer::Client)
        else {
            panic!("the first SUBSCRIBE starts the creation");
        };
        assert!(matches!(
            table.track_or_begin_creation(&track_key(), SessionPeer::Client),
            TrackLookup::Creating(..)
        ));
        assert!(table.track_state(&track_key()).is_none());
        table.add_pending_producers(&track_key(), 1);

        // Act: the first producer completes the creation without a consumer
        let taken = table.add_upstream_subscription_to_track(
            &track_key(),
            PUBLISHER_SESSION,
            producer(ProducerOrigin::Subscribe),
        );

        // Assert
        assert!(taken);
        assert_eq!(*creation.borrow(), CreationState::Created);
        assert!(matches!(
            table.track_or_begin_creation(&track_key(), SessionPeer::Client),
            TrackLookup::Ready(..)
        ));
    }

    #[tokio::test]
    async fn a_creation_whose_subscribes_all_fail_ends_with_the_last_failure() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        let TrackLookup::Started(creation, _reservation) =
            table.track_or_begin_creation(&track_key(), SessionPeer::Client)
        else {
            panic!("the first SUBSCRIBE starts the creation");
        };
        table.add_pending_producers(&track_key(), 2);

        // Act
        table.pending_producer_ended(&track_key(), Some(creation_failure("first")));
        let kept_after_first = table.upstream_tracks.contains_key(&track_key());
        table.pending_producer_ended(&track_key(), Some(creation_failure("last")));

        // Assert
        assert!(kept_after_first);
        assert_eq!(
            *creation.borrow(),
            CreationState::Failed(creation_failure("last"))
        );
        assert!(table.upstream_tracks.is_empty());
    }

    #[tokio::test]
    async fn a_publish_during_the_creation_completes_it() {
        // Arrange
        let table = Arc::new(InMemoryLocalPubSubDirectory::new());
        let TrackLookup::Started(creation, _reservation) =
            table.track_or_begin_creation(&track_key(), SessionPeer::Client)
        else {
            panic!("the first SUBSCRIBE starts the creation");
        };
        table.add_pending_producers(&track_key(), 1);

        // Act
        table.register_upstream_subscription(
            track_key(),
            PUBLISHER_SESSION,
            producer(ProducerOrigin::Publish),
        );
        table.pending_producer_ended(&track_key(), Some(creation_failure("refused")));

        // Assert
        assert_eq!(*creation.borrow(), CreationState::Created);
        assert!(table.track_state(&track_key()).is_some());
    }

    #[tokio::test]
    async fn a_reserved_track_wants_producers_before_its_consumer_registers() {
        // Arrange
        let (table, _) = table_with_upstream(ProducerOrigin::Subscribe);
        let table = Arc::new(table);
        let TrackLookup::Ready(_, reservation) =
            table.track_or_begin_creation(&track_key(), SessionPeer::Client)
        else {
            panic!("a fed track is ready");
        };

        // Act / Assert
        assert!(table.track_takes_publisher(&track_key(), 3, SessionPeer::Client));
        drop(reservation);
        assert!(!table.track_takes_publisher(&track_key(), 3, SessionPeer::Client));
    }

    #[tokio::test]
    async fn a_forwarded_publish_in_flight_blocks_a_second_one_to_the_session() {
        // Arrange
        let (table, _) = table_with_upstream(ProducerOrigin::Publish);
        let table = Arc::new(table);

        // Act
        let forwarding = table.begin_forwarded_publish(&track_key(), 2);
        let second = table.begin_forwarded_publish(&track_key(), 2);

        // Assert
        assert!(forwarding.is_some());
        assert!(second.is_none());
        drop(forwarding);
        assert!(table.begin_forwarded_publish(&track_key(), 2).is_some());
    }
}

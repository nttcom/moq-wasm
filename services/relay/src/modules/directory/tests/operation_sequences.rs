//! Checks the directory's bookkeeping against a model after every
//! operation, over random sequences of the calls the control plane makes on
//! it. A SUBSCRIBE and a PUBLISH run in steps, as their handlers await
//! between their calls, and each upstream SUBSCRIBE of a
//! `PendingProducerTask` is claimed and answered in steps of its own, so
//! other sessions' calls interleave with them.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use moqt::{RequestErrorCode, namespace::NamespaceInterest};
use proptest::{collection::vec, prelude::*, sample::Index};
use tokio::sync::{
    oneshot::{self, error::TryRecvError},
    watch,
};

use crate::modules::{
    control_plane::sequences::subscribe::UPSTREAM_ATTEMPTS,
    directory::{
        InMemoryLocalPubSubDirectory, TrackEntry, TrackLookup, TrackReservation,
        track::{
            CreationFailure, CreationState, NamespaceSubscription, ProducerOrigin, ProducerState,
            RemovedDownstreamSubscription,
        },
    },
    domain::{
        publish_done_reason::PublishDoneReason,
        session_id::SessionId,
        session_peer::SessionPeer,
        track_key::{TrackKey, namespace_has_prefix},
    },
    test_support::{
        directory_fixtures::{
            active_upstream, delivery_control, downstream_subscription, producer_with,
        },
        relay_harness::fixtures::subscription::upstream_subscription,
    },
};

const NAMESPACES: [&str; 2] = ["room", "lobby"];
/// Each track as the index of its namespace and its name.
const TRACKS: [(usize, &str); 3] = [(0, "video"), (0, "audio"), (1, "video")];
/// The empty prefix covers both namespaces, so it overlaps the others.
const PREFIXES: [&str; 3] = ["room", "lobby", ""];
const INTERESTS: [NamespaceInterest; 3] = [
    NamespaceInterest::Namespaces,
    NamespaceInterest::Tracks,
    NamespaceInterest::NamespacesAndTracks,
];
/// A slot whose session was removed connects again under a new, larger
/// session id.
const SLOT_PEERS: [SessionPeer; 3] = [SessionPeer::Client, SessionPeer::Client, SessionPeer::Relay];

fn track_key(track: usize) -> TrackKey {
    let (namespace, name) = TRACKS[track];
    TrackKey::new(NAMESPACES[namespace], name)
}

fn track_of(track_key: &TrackKey) -> usize {
    (0..TRACKS.len())
        .find(|track| self::track_key(*track) == *track_key)
        .unwrap_or_else(|| unreachable!("the directory holds only the tracks the test made"))
}

fn interest_index(interest: NamespaceInterest) -> usize {
    (0..INTERESTS.len())
        .find(|index| INTERESTS[*index] == interest)
        .unwrap_or_else(|| unreachable!("INTERESTS lists every interest"))
}

fn creation_failure() -> CreationFailure {
    CreationFailure {
        code: RequestErrorCode::TrackDoesNotExist,
        reason: "refused".to_string(),
    }
}

#[derive(Clone, Debug)]
enum Operation {
    PublishNamespace {
        slot: usize,
        namespace: usize,
    },
    PublishNamespaceDone {
        slot: usize,
        namespace: usize,
    },
    SubscribeNamespace {
        slot: usize,
        prefix: usize,
        interest: usize,
    },
    UnsubscribeNamespace {
        pick: Index,
    },
    /// The first step of a PUBLISH: its upstream subscription is registered.
    Publish {
        slot: usize,
        track: usize,
        on_demand: bool,
    },
    /// The first step of a SUBSCRIBE: the track is looked up or created.
    Subscribe {
        slot: usize,
        track: usize,
        forward: bool,
    },
    /// The next step of the slot's SUBSCRIBE, or the end of its PUBLISH.
    Advance {
        slot: usize,
    },
    /// The next step of an upstream SUBSCRIBE: its claim, then its answer.
    AnswerUpstream {
        pick: Index,
        accepted: bool,
    },
    PublishDone {
        pick: Index,
    },
    /// UNSUBSCRIBE, or the relay ending the subscription with PUBLISH_DONE.
    Unsubscribe {
        pick: Index,
        publish_done: bool,
    },
    /// The relay forwards a track's PUBLISH to a namespace subscriber.
    ForwardPublish {
        slot: usize,
        track: usize,
        blocked: bool,
    },
    RemoveSession {
        slot: usize,
    },
    BeginDraining,
    GoAway {
        slot: usize,
    },
    MalformedTrack {
        track: usize,
    },
}

fn operation() -> impl Strategy<Value = Operation> {
    let slot = || 0..SLOT_PEERS.len();
    let track = || 0..TRACKS.len();
    prop_oneof![
        2 => (slot(), 0..NAMESPACES.len())
            .prop_map(|(slot, namespace)| Operation::PublishNamespace { slot, namespace }),
        1 => (slot(), 0..NAMESPACES.len())
            .prop_map(|(slot, namespace)| Operation::PublishNamespaceDone { slot, namespace }),
        2 => (slot(), 0..PREFIXES.len(), 0..INTERESTS.len()).prop_map(
            |(slot, prefix, interest)| Operation::SubscribeNamespace { slot, prefix, interest }
        ),
        1 => any::<Index>().prop_map(|pick| Operation::UnsubscribeNamespace { pick }),
        3 => (slot(), track(), any::<bool>())
            .prop_map(|(slot, track, on_demand)| Operation::Publish { slot, track, on_demand }),
        4 => (slot(), track(), any::<bool>())
            .prop_map(|(slot, track, forward)| Operation::Subscribe { slot, track, forward }),
        8 => slot().prop_map(|slot| Operation::Advance { slot }),
        6 => (any::<Index>(), prop::bool::weighted(0.75))
            .prop_map(|(pick, accepted)| Operation::AnswerUpstream { pick, accepted }),
        2 => any::<Index>().prop_map(|pick| Operation::PublishDone { pick }),
        3 => (any::<Index>(), any::<bool>())
            .prop_map(|(pick, publish_done)| Operation::Unsubscribe { pick, publish_done }),
        2 => (slot(), track(), prop::bool::weighted(0.25))
            .prop_map(|(slot, track, blocked)| Operation::ForwardPublish { slot, track, blocked }),
        2 => slot().prop_map(|slot| Operation::RemoveSession { slot }),
        1 => Just(Operation::BeginDraining),
        1 => slot().prop_map(|slot| Operation::GoAway { slot }),
        1 => track().prop_map(|track| Operation::MalformedTrack { track }),
    ]
}

enum SubscribeStage {
    /// The SUBSCRIBE created the track and is to resolve its publishers.
    Starting(watch::Receiver<CreationState>, TrackReservation),
    Waiting(watch::Receiver<CreationState>, TrackReservation),
    /// The SUBSCRIBE found the track and is to register its consumer.
    Ready(TrackReservation),
}

struct PendingSubscribe {
    track: usize,
    request_id: u64,
    forward: bool,
    registrations: usize,
    stage: SubscribeStage,
}

struct Slot {
    session_id: SessionId,
    peer: SessionPeer,
    /// The SUBSCRIBE the session's worker is handling; it handles no other
    /// event meanwhile.
    in_flight: Option<PendingSubscribe>,
    /// Whether the worker is between the two steps of a PUBLISH: its upstream
    /// subscription is registered and it awaits the UNSUBSCRIBE of the
    /// subscription it replaced before it sends PUBLISH_OK.
    publishing: bool,
}

struct UpstreamSubscribe {
    track: usize,
    entry: TrackEntry,
    publisher_session_id: SessionId,
    publisher_peer: SessionPeer,
    claimed: bool,
}

struct ModelConsumer {
    subscriber_session_id: SessionId,
    request_id: u64,
    track: usize,
    stop_receiver: oneshot::Receiver<PublishDoneReason>,
}

/// A consumer the step may drop without PUBLISH_DONE: its subscriber ended
/// it or left, or its track turned out malformed and its runner ends on its
/// own.
enum SilentEnd {
    Consumer(SessionId, u64),
    Subscriber(SessionId),
    Track(usize),
}

impl SilentEnd {
    fn allows(&self, consumer: &ModelConsumer) -> bool {
        match self {
            Self::Consumer(session_id, request_id) => {
                consumer.subscriber_session_id == *session_id && consumer.request_id == *request_id
            }
            Self::Subscriber(session_id) => consumer.subscriber_session_id == *session_id,
            Self::Track(track) => consumer.track == *track,
        }
    }
}

struct Harness {
    directory: Arc<InMemoryLocalPubSubDirectory>,
    slots: Vec<Slot>,
    next_session_id: SessionId,
    next_request_id: u64,
    publish_namespaces: BTreeMap<(SessionId, usize), SessionPeer>,
    /// (session, prefix, interest) with the subscription's peer.
    namespace_subscriptions: BTreeMap<(SessionId, usize, usize), SessionPeer>,
    /// The upstream subscriptions, by track and publisher session.
    producers: BTreeMap<(usize, SessionId), (u64, ProducerOrigin)>,
    consumers: Vec<ModelConsumer>,
    upstream_subscribes: Vec<UpstreamSubscribe>,
    blocked_publishes: BTreeSet<(SessionId, usize)>,
    malformed_tracks: BTreeSet<usize>,
    draining: bool,
}

impl Harness {
    fn new() -> Self {
        Self {
            directory: Arc::new(InMemoryLocalPubSubDirectory::new()),
            slots: SLOT_PEERS
                .iter()
                .enumerate()
                .map(|(index, peer)| Slot {
                    session_id: index as SessionId + 1,
                    peer: *peer,
                    in_flight: None,
                    publishing: false,
                })
                .collect(),
            next_session_id: SLOT_PEERS.len() as SessionId + 1,
            next_request_id: 1,
            publish_namespaces: BTreeMap::new(),
            namespace_subscriptions: BTreeMap::new(),
            producers: BTreeMap::new(),
            consumers: Vec::new(),
            upstream_subscribes: Vec::new(),
            blocked_publishes: BTreeSet::new(),
            malformed_tracks: BTreeSet::new(),
            draining: false,
        }
    }

    fn request_id(&mut self) -> u64 {
        self.next_request_id += 1;
        self.next_request_id
    }

    /// The slot's session when its worker can take an event.
    fn idle_session(&self, slot: usize) -> Option<(SessionId, SessionPeer)> {
        let slot = &self.slots[slot];
        (slot.in_flight.is_none() && !slot.publishing).then_some((slot.session_id, slot.peer))
    }

    fn is_idle_session(&self, session_id: SessionId) -> bool {
        self.slots.iter().any(|slot| {
            slot.session_id == session_id && slot.in_flight.is_none() && !slot.publishing
        })
    }

    fn remove_producer(
        &mut self,
        track: usize,
        publisher_session_id: SessionId,
        request_id: u64,
        origin: ProducerOrigin,
    ) -> Result<(), TestCaseError> {
        // The origin decides whether the caller withdraws the track's route.
        let removed = self.producers.remove(&(track, publisher_session_id));
        prop_assert_eq!(
            removed,
            Some((request_id, origin)),
            "the directory released upstream subscription {} of session {} on track {}, which the model does not hold",
            request_id,
            publisher_session_id,
            track
        );
        Ok(())
    }

    fn remove_released(
        &mut self,
        removed: &[RemovedDownstreamSubscription],
    ) -> Result<(), TestCaseError> {
        for removed in removed {
            for released in &removed.released_upstream_subscriptions {
                self.remove_producer(
                    track_of(&removed.track_key),
                    released.publisher_session_id,
                    released.subscription.request_id(),
                    released.origin,
                )?;
            }
        }
        Ok(())
    }

    fn subscribe_publishers(
        &mut self,
        entry: &TrackEntry,
        publishers: Vec<(SessionId, SessionPeer)>,
    ) {
        if publishers.is_empty() {
            return;
        }
        self.directory
            .add_pending_producers(entry, publishers.len());
        for (publisher_session_id, publisher_peer) in publishers {
            self.upstream_subscribes.push(UpstreamSubscribe {
                track: track_of(&entry.track_key),
                entry: entry.clone(),
                publisher_session_id,
                publisher_peer,
                claimed: false,
            });
        }
    }

    /// The relay sessions a route registry would list for the namespace: the
    /// relays publishing it here.
    fn publisher_relays(&self, namespace: usize) -> Vec<(SessionId, SessionPeer)> {
        self.publish_namespaces
            .iter()
            .filter(|((_, published), peer)| {
                *published == namespace && **peer == SessionPeer::Relay
            })
            .map(|((session_id, _), peer)| (*session_id, *peer))
            .collect()
    }

    fn peer_of(&self, session_id: SessionId) -> Option<SessionPeer> {
        self.slots
            .iter()
            .find(|slot| slot.session_id == session_id)
            .map(|slot| slot.peer)
    }

    /// The top of `get_or_create_upstream_subscription`'s loop; `None` when
    /// the SUBSCRIBE is answered with an error.
    fn look_up(&self, track: usize, peer: SessionPeer) -> Option<SubscribeStage> {
        if self.malformed_tracks.contains(&track) {
            return None;
        }
        Some(
            match self
                .directory
                .track_or_begin_creation(&track_key(track), peer)
            {
                TrackLookup::Ready(_, reservation) => SubscribeStage::Ready(reservation),
                TrackLookup::Creating(creation, reservation) => {
                    SubscribeStage::Waiting(creation, reservation)
                }
                TrackLookup::Started(creation, reservation) => {
                    SubscribeStage::Starting(creation, reservation)
                }
            },
        )
    }

    fn apply(&mut self, operation: &Operation) -> Result<Vec<SilentEnd>, TestCaseError> {
        match *operation {
            Operation::PublishNamespace { slot, namespace } => {
                let Some((session_id, peer)) = self.idle_session(slot) else {
                    return Ok(Vec::new());
                };
                self.directory.register_publish_namespace(
                    session_id,
                    0,
                    NAMESPACES[namespace].to_string(),
                    peer,
                );
                self.publish_namespaces
                    .insert((session_id, namespace), peer);
                // `join_tracks_wanting_publishers`
                let (publishers, joining) = match peer {
                    SessionPeer::Client => (vec![(session_id, peer)], Some(session_id)),
                    SessionPeer::Relay => (self.publisher_relays(namespace), None),
                };
                for entry in
                    self.directory
                        .tracks_wanting_publisher(NAMESPACES[namespace], peer, joining)
                {
                    self.subscribe_publishers(&entry, publishers.clone());
                }
            }
            Operation::PublishNamespaceDone { slot, namespace } => {
                let Some((session_id, _)) = self.idle_session(slot) else {
                    return Ok(Vec::new());
                };
                if self
                    .publish_namespaces
                    .remove(&(session_id, namespace))
                    .is_none()
                {
                    return Ok(Vec::new());
                }
                self.directory.unregister_publish_namespace(
                    session_id,
                    None,
                    NAMESPACES[namespace],
                );
            }
            Operation::SubscribeNamespace {
                slot,
                prefix,
                interest,
            } => {
                let Some((session_id, peer)) = self.idle_session(slot) else {
                    return Ok(Vec::new());
                };
                if self.directory.has_overlapping_namespace_subscription(
                    session_id,
                    PREFIXES[prefix],
                    INTERESTS[interest],
                    None,
                ) {
                    return Ok(Vec::new());
                }
                self.directory.register_subscribe_namespace(
                    session_id,
                    PREFIXES[prefix].to_string(),
                    NamespaceSubscription {
                        interest: INTERESTS[interest],
                        peer,
                        forward: true,
                    },
                );
                self.namespace_subscriptions
                    .insert((session_id, prefix, interest), peer);
            }
            Operation::UnsubscribeNamespace { pick } => {
                let held: Vec<_> = self
                    .namespace_subscriptions
                    .iter()
                    .filter(|((session_id, _, _), _)| self.is_idle_session(*session_id))
                    .map(|(key, peer)| (*key, *peer))
                    .collect();
                if held.is_empty() {
                    return Ok(Vec::new());
                }
                let ((session_id, prefix, interest), peer) = held[pick.index(held.len())];
                let no_clients_remain = self.directory.unregister_subscribe_namespace(
                    session_id,
                    PREFIXES[prefix],
                    INTERESTS[interest],
                );
                self.namespace_subscriptions
                    .remove(&(session_id, prefix, interest));
                if INTERESTS[interest].includes_tracks() {
                    self.blocked_publishes.retain(|(subscriber, track)| {
                        *subscriber != session_id
                            || !namespace_has_prefix(NAMESPACES[TRACKS[*track].0], PREFIXES[prefix])
                    });
                }
                if peer == SessionPeer::Client && no_clients_remain {
                    self.directory
                        .purge_relay_publish_namespaces(PREFIXES[prefix]);
                    let watched_by_client = |namespace: usize| {
                        self.namespace_subscriptions
                            .iter()
                            .any(|((_, prefix, _), peer)| {
                                *peer == SessionPeer::Client
                                    && namespace_has_prefix(
                                        NAMESPACES[namespace],
                                        PREFIXES[*prefix],
                                    )
                            })
                    };
                    let purged: Vec<_> = self
                        .publish_namespaces
                        .iter()
                        .filter(|((_, namespace), peer)| {
                            **peer == SessionPeer::Relay
                                && namespace_has_prefix(NAMESPACES[*namespace], PREFIXES[prefix])
                                && !watched_by_client(*namespace)
                        })
                        .map(|(key, _)| *key)
                        .collect();
                    for key in purged {
                        self.publish_namespaces.remove(&key);
                    }
                }
            }
            Operation::Publish {
                slot,
                track,
                on_demand,
            } => {
                let Some((session_id, peer)) = self.idle_session(slot) else {
                    return Ok(Vec::new());
                };
                let request_id = self.request_id();
                let producer = producer_with(ProducerState {
                    subscription: upstream_subscription(request_id),
                    peer,
                    on_demand,
                    ..active_upstream(ProducerOrigin::Publish)
                });
                let (_, replaced) = self.directory.register_published_subscription(
                    track_key(track),
                    session_id,
                    producer,
                );
                let expected = self
                    .producers
                    .insert((track, session_id), (request_id, ProducerOrigin::Publish))
                    .map(|(request_id, _)| request_id);
                prop_assert_eq!(
                    replaced.map(|replaced| replaced.state.subscription.request_id()),
                    expected,
                    "PUBLISH replaced another upstream subscription than the session's earlier one"
                );
                self.slots[slot].publishing = true;
            }
            Operation::Subscribe {
                slot,
                track,
                forward,
            } => {
                let Some((_, peer)) = self.idle_session(slot) else {
                    return Ok(Vec::new());
                };
                let request_id = self.request_id();
                if let Some(stage) = self.look_up(track, peer) {
                    self.slots[slot].in_flight = Some(PendingSubscribe {
                        track,
                        request_id,
                        forward,
                        registrations: 0,
                        stage,
                    });
                }
            }
            Operation::Advance { slot } => {
                if self.slots[slot].publishing {
                    self.slots[slot].publishing = false;
                    return Ok(Vec::new());
                }
                let session_id = self.slots[slot].session_id;
                let peer = self.slots[slot].peer;
                let pending = self.slots[slot].in_flight.take();
                self.slots[slot].in_flight =
                    pending.and_then(|pending| self.advance_subscribe(session_id, peer, pending));
            }
            Operation::AnswerUpstream { pick, accepted } => {
                if self.upstream_subscribes.is_empty() {
                    return Ok(Vec::new());
                }
                let index = pick.index(self.upstream_subscribes.len());
                self.answer_upstream(index, accepted);
            }
            Operation::PublishDone { pick } => {
                let live: Vec<_> = self
                    .producers
                    .iter()
                    .filter(|((_, publisher), _)| self.is_idle_session(*publisher))
                    .map(|((track, publisher), (request_id, _))| (*track, *publisher, *request_id))
                    .collect();
                if live.is_empty() {
                    return Ok(Vec::new());
                }
                let (track, publisher_session_id, request_id) = live[pick.index(live.len())];
                let ended = self.directory.end_upstream_subscription(
                    publisher_session_id,
                    request_id,
                    PublishDoneReason::publisher_session_closed(),
                );
                prop_assert_eq!(ended, Some(track_key(track)));
                self.producers.remove(&(track, publisher_session_id));
            }
            Operation::Unsubscribe { pick, publish_done } => {
                let live: Vec<_> = self
                    .consumers
                    .iter()
                    .filter(|consumer| self.is_idle_session(consumer.subscriber_session_id))
                    .map(|consumer| (consumer.subscriber_session_id, consumer.request_id))
                    .collect();
                if live.is_empty() {
                    return Ok(Vec::new());
                }
                let (session_id, request_id) = live[pick.index(live.len())];
                let end = publish_done.then(PublishDoneReason::subscription_ended);
                let removed = self
                    .directory
                    .end_downstream_subscription(session_id, request_id, end);
                prop_assert!(
                    removed.is_some(),
                    "downstream subscription {request_id} of session {session_id} was not registered"
                );
                self.remove_released(removed.as_slice())?;
                if !publish_done {
                    return Ok(vec![SilentEnd::Consumer(session_id, request_id)]);
                }
            }
            Operation::ForwardPublish {
                slot,
                track,
                blocked,
            } => self.forward_publish(slot, track, blocked),
            Operation::RemoveSession { slot } => {
                let Some((session_id, _)) = self.idle_session(slot) else {
                    return Ok(Vec::new());
                };
                let removed = self.directory.remove_session(session_id);
                self.remove_released(&removed.downstream_subscriptions)?;
                self.producers
                    .retain(|(_, publisher), _| *publisher != session_id);
                self.publish_namespaces
                    .retain(|(publisher, _), _| *publisher != session_id);
                self.namespace_subscriptions
                    .retain(|(subscriber, _, _), _| *subscriber != session_id);
                self.blocked_publishes
                    .retain(|(subscriber, _)| *subscriber != session_id);
                self.check_session_forgotten(session_id)?;
                self.slots[slot].session_id = self.next_session_id;
                self.next_session_id += 1;
                return Ok(vec![SilentEnd::Subscriber(session_id)]);
            }
            Operation::BeginDraining => {
                self.directory.begin_draining(0);
                self.draining = true;
            }
            Operation::GoAway { slot } => {
                let Some((session_id, _)) = self.idle_session(slot) else {
                    return Ok(Vec::new());
                };
                if !self.draining {
                    return Ok(Vec::new());
                }
                for (track_key, released) in self
                    .directory
                    .release_unwatched_published_subscriptions(session_id)
                {
                    prop_assert_eq!(released.publisher_session_id, session_id);
                    self.remove_producer(
                        track_of(&track_key),
                        session_id,
                        released.subscription.request_id(),
                        released.origin,
                    )?;
                }
            }
            Operation::MalformedTrack { track } => {
                if !self
                    .directory
                    .upstream_tracks
                    .contains_key(&track_key(track))
                {
                    return Ok(Vec::new());
                }
                for (publisher_session_id, producer) in
                    self.directory.remove_upstream_track(&track_key(track))
                {
                    self.remove_producer(
                        track,
                        publisher_session_id,
                        producer.state.subscription.request_id(),
                        producer.state.origin,
                    )?;
                }
                self.malformed_tracks.insert(track);
                return Ok(vec![SilentEnd::Track(track)]);
            }
        }
        Ok(Vec::new())
    }

    /// One step of `Subscribe::handle`; `None` once the SUBSCRIBE is answered.
    fn advance_subscribe(
        &mut self,
        session_id: SessionId,
        peer: SessionPeer,
        mut pending: PendingSubscribe,
    ) -> Option<PendingSubscribe> {
        let track = pending.track;
        let track_key = track_key(track);
        pending.stage = match pending.stage {
            // `create_upstream_subscription`
            SubscribeStage::Starting(creation, reservation) => {
                let (namespace, name) = TRACKS[track];
                let mut publishers: Vec<_> = self
                    .directory
                    .find_upstream_publishers(NAMESPACES[namespace], name)
                    .into_iter()
                    .rev()
                    .map(|publisher| (publisher, SessionPeer::Client))
                    .collect();
                if peer == SessionPeer::Client {
                    publishers.extend(self.publisher_relays(namespace));
                }
                if publishers.is_empty() {
                    self.directory
                        .pending_producer_ended(reservation.entry(), Some(creation_failure()));
                } else {
                    self.subscribe_publishers(reservation.entry(), publishers);
                }
                SubscribeStage::Waiting(creation, reservation)
            }
            SubscribeStage::Waiting(creation, reservation) => {
                let state = creation.borrow().clone();
                let creation_dropped = creation.has_changed().is_err();
                match state {
                    CreationState::Creating if !creation_dropped => {
                        SubscribeStage::Waiting(creation, reservation)
                    }
                    CreationState::Failed(_) => return None,
                    _ => {
                        if self.directory.entry_state(reservation.entry()).is_some() {
                            SubscribeStage::Ready(reservation)
                        } else {
                            drop(reservation);
                            self.look_up(track, peer)?
                        }
                    }
                }
            }
            // `accept_downstream_subscription`
            SubscribeStage::Ready(reservation) => {
                if self.malformed_tracks.contains(&track) {
                    return None;
                }
                let registered = self.directory.register_downstream_subscription(
                    reservation.entry(),
                    session_id,
                    pending.request_id,
                    peer,
                    downstream_subscription(track_key, None),
                    delivery_control(pending.forward),
                );
                drop(reservation);
                if let Some(signals) = registered {
                    self.consumers.push(ModelConsumer {
                        subscriber_session_id: session_id,
                        request_id: pending.request_id,
                        track,
                        stop_receiver: signals.stop_receiver,
                    });
                    return None;
                }
                pending.registrations += 1;
                if pending.registrations == UPSTREAM_ATTEMPTS {
                    return None;
                }
                self.look_up(track, peer)?
            }
        };
        Some(pending)
    }

    /// One step of a `PendingProducerTask`.
    fn answer_upstream(&mut self, index: usize, accepted: bool) {
        let upstream = &mut self.upstream_subscribes[index];
        let track_key = track_key(upstream.track);
        let publisher_session_id = upstream.publisher_session_id;
        if !upstream.claimed {
            if self
                .directory
                .begin_upstream_subscribe(&upstream.entry, publisher_session_id)
            {
                upstream.claimed = true;
            } else {
                let upstream = self.upstream_subscribes.remove(index);
                self.directory.pending_producer_ended(&upstream.entry, None);
            }
            return;
        }
        let upstream = self.upstream_subscribes.remove(index);
        // A removed session answers no SUBSCRIBE.
        let failure = if accepted && self.peer_of(publisher_session_id).is_some() {
            if self
                .directory
                .published_upstream_subscription(&track_key, publisher_session_id)
                .is_none()
                && self.directory.track_takes_publisher(
                    &upstream.entry,
                    publisher_session_id,
                    upstream.publisher_peer,
                )
            {
                let request_id = self.request_id();
                let producer = producer_with(ProducerState {
                    subscription: upstream_subscription(request_id),
                    peer: upstream.publisher_peer,
                    ..active_upstream(ProducerOrigin::Subscribe)
                });
                if self.directory.add_upstream_subscription_to_track(
                    &upstream.entry,
                    publisher_session_id,
                    producer,
                ) {
                    self.producers.insert(
                        (upstream.track, publisher_session_id),
                        (request_id, ProducerOrigin::Subscribe),
                    );
                }
            }
            None
        } else {
            Some(creation_failure())
        };
        self.directory
            .end_upstream_subscribe(&upstream.entry, publisher_session_id);
        self.directory
            .pending_producer_ended(&upstream.entry, failure);
    }

    /// `DownstreamPublish::send` to a namespace subscriber the relay offers
    /// the track to.
    fn forward_publish(&mut self, slot: usize, track: usize, blocked: bool) {
        let Slot {
            session_id, peer, ..
        } = self.slots[slot];
        let track_key = track_key(track);
        let Some(subscription) = self
            .directory
            .get_namespace_subscribers(&track_key.track_namespace, NamespaceInterest::Tracks)
            .remove(&session_id)
        else {
            return;
        };
        if !self
            .directory
            .offers_track(session_id, &subscription, &track_key)
            || self.directory.track_state(&track_key).is_none()
            || self.directory.is_publish_blocked(session_id, &track_key)
        {
            return;
        }
        let Some(forwarding) = self
            .directory
            .begin_forwarded_publish(&track_key, session_id)
        else {
            return;
        };
        // PUBLISH_BLOCKED is a draft-18 answer, to a SUBSCRIBE_TRACKS.
        if blocked && subscription.interest == NamespaceInterest::Tracks {
            self.directory.block_publish(session_id, &track_key);
            self.blocked_publishes.insert((session_id, track));
        } else {
            let request_id = self.request_id();
            if let Some(signals) = self.directory.register_downstream_subscription(
                forwarding.entry(),
                session_id,
                request_id,
                peer,
                downstream_subscription(track_key, None),
                delivery_control(subscription.forward),
            ) {
                self.consumers.push(ModelConsumer {
                    subscriber_session_id: session_id,
                    request_id,
                    track,
                    stop_receiver: signals.stop_receiver,
                });
            }
        }
    }

    /// Nothing in the directory refers to a removed session.
    fn check_session_forgotten(&self, session_id: SessionId) -> Result<(), TestCaseError> {
        let directory = &self.directory;
        for entry in directory.upstream_tracks.iter() {
            prop_assert!(
                !entry.producers.contains_key(&session_id),
                "removed session {session_id} still feeds {}",
                entry.key()
            );
            prop_assert!(
                !entry
                    .consumers
                    .keys()
                    .any(|(subscriber, _)| *subscriber == session_id),
                "removed session {session_id} still receives {}",
                entry.key()
            );
        }
        let indexed = directory
            .consumer_tracks
            .iter()
            .any(|entry| entry.key().0 == session_id);
        prop_assert!(
            !indexed,
            "removed session {session_id} still indexes a consumer"
        );
        let publishes_namespace = directory.publisher_namespaces.iter().any(|entry| {
            entry
                .value()
                .iter()
                .any(|publication| publication.key().0 == session_id)
        });
        prop_assert!(
            !publishes_namespace,
            "removed session {session_id} still publishes a namespace"
        );
        let subscribes_namespace = directory.subscriber_namespaces.iter().any(|entry| {
            entry
                .value()
                .iter()
                .any(|subscription| subscription.key().0 == session_id)
        });
        prop_assert!(
            !subscribes_namespace,
            "removed session {session_id} still subscribes a namespace"
        );
        let blocked = directory
            .blocked_publishes()
            .iter()
            .any(|(subscriber, _)| *subscriber == session_id);
        prop_assert!(
            !blocked,
            "removed session {session_id} still has a blocked PUBLISH"
        );
        Ok(())
    }

    /// An upstream SUBSCRIBE for the track has yet to end, or the SUBSCRIBE
    /// creating it has yet to resolve its publishers.
    fn awaits_producer(&self, track: usize) -> bool {
        self.upstream_subscribes
            .iter()
            .any(|upstream| upstream.track == track)
            || self.slots.iter().any(|slot| {
                matches!(
                    &slot.in_flight,
                    Some(PendingSubscribe {
                        track: pending_track,
                        stage: SubscribeStage::Starting(..),
                        ..
                    }) if *pending_track == track
                )
            })
    }

    fn check(&mut self, silent_ends: &[SilentEnd]) -> Result<(), TestCaseError> {
        let directory = self.directory.clone();

        // Every upstream subscription the directory drops is handed to its
        // caller, which unsubscribes it; none appears unasked.
        let producers: BTreeMap<_, _> = directory
            .upstream_tracks
            .iter()
            .flat_map(|entry| {
                let track = track_of(entry.key());
                entry
                    .producers
                    .iter()
                    .map(|(publisher, producer)| {
                        (
                            (track, *publisher),
                            (
                                producer.state.subscription.request_id(),
                                producer.state.origin,
                            ),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        prop_assert_eq!(producers, self.producers.clone(), "upstream subscriptions");

        // The upstream publishers of a track are the clients publishing its
        // namespace or PUBLISHing it.
        for (track, (namespace, name)) in TRACKS.into_iter().enumerate() {
            let expected: BTreeSet<_> = self
                .publish_namespaces
                .iter()
                .filter(|((_, published), peer)| {
                    *published == namespace && **peer == SessionPeer::Client
                })
                .map(|((session_id, _), _)| *session_id)
                .chain(
                    self.producers
                        .iter()
                        .filter(|((produced, publisher), (_, origin))| {
                            *produced == track
                                && *origin == ProducerOrigin::Publish
                                && self.peer_of(*publisher) == Some(SessionPeer::Client)
                        })
                        .map(|((_, publisher), _)| *publisher),
                )
                .collect();
            prop_assert_eq!(
                directory.find_upstream_publishers(NAMESPACES[namespace], name),
                expected.into_iter().collect::<Vec<_>>(),
                "upstream publishers of {}",
                track_key(track)
            );
        }

        let publish_namespaces: BTreeMap<_, _> = directory
            .publisher_namespaces
            .iter()
            .flat_map(|entry| {
                let namespace = NAMESPACES
                    .iter()
                    .position(|namespace| namespace == entry.key())
                    .unwrap_or_else(|| unreachable!("only NAMESPACES are published"));
                entry
                    .value()
                    .iter()
                    .map(|publisher| ((publisher.key().0, namespace), *publisher.value()))
                    .collect::<Vec<_>>()
            })
            .collect();
        prop_assert_eq!(&publish_namespaces, &self.publish_namespaces);

        let namespace_subscriptions: BTreeMap<_, _> = directory
            .subscriber_namespaces
            .iter()
            .flat_map(|entry| {
                let prefix = PREFIXES
                    .iter()
                    .position(|prefix| prefix == entry.key())
                    .unwrap_or_else(|| unreachable!("only PREFIXES are subscribed"));
                entry
                    .value()
                    .iter()
                    .map(|subscription| {
                        let (session_id, interest) = *subscription.key();
                        (
                            (session_id, prefix, interest_index(interest)),
                            subscription.value().peer,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        prop_assert_eq!(&namespace_subscriptions, &self.namespace_subscriptions);

        // A PUBLISH stays blocked while the namespace subscription it was blocked
        // on lasts.
        let blocked: BTreeSet<_> = directory
            .blocked_publishes()
            .iter()
            .map(|(session_id, track_key)| (*session_id, track_of(track_key)))
            .collect();
        prop_assert_eq!(&blocked, &self.blocked_publishes);

        // A consumer leaves the directory only with PUBLISH_DONE, unless its
        // subscriber ended it or left, or its track is malformed.
        let consumers: BTreeMap<_, _> = directory
            .upstream_tracks
            .iter()
            .flat_map(|entry| {
                let track = track_of(entry.key());
                entry
                    .consumers
                    .keys()
                    .map(|key| (*key, track))
                    .collect::<Vec<_>>()
            })
            .collect();
        let mut live = BTreeMap::new();
        for mut consumer in std::mem::take(&mut self.consumers) {
            let key = (consumer.subscriber_session_id, consumer.request_id);
            let registered = consumers.get(&key) == Some(&consumer.track);
            match consumer.stop_receiver.try_recv() {
                Err(TryRecvError::Empty) => {
                    prop_assert!(registered, "consumer {key:?} left with its runner running");
                    live.insert(key, consumer.track);
                    self.consumers.push(consumer);
                }
                Ok(_) => prop_assert!(!registered, "consumer {key:?} ended but registered"),
                Err(TryRecvError::Closed) => {
                    prop_assert!(!registered, "consumer {key:?} dropped but registered");
                    prop_assert!(
                        silent_ends.iter().any(|end| end.allows(&consumer)),
                        "consumer {key:?} of {} dropped without PUBLISH_DONE",
                        track_key(consumer.track)
                    );
                }
            }
        }
        prop_assert_eq!(&consumers, &live, "downstream subscriptions");

        for entry in directory.upstream_tracks.iter() {
            let track = track_of(entry.key());
            for key in entry.consumers.keys() {
                prop_assert_eq!(
                    directory
                        .consumer_tracks
                        .get(key)
                        .map(|indexed| indexed.clone()),
                    Some(entry.key().clone()),
                    "consumer {:?} is indexed under another track",
                    key
                );
            }
            // A track is fed, or a producer is still on its way to it:
            // otherwise its consumers never get an object or PUBLISH_DONE.
            prop_assert!(
                !entry.producers.is_empty() || self.awaits_producer(track),
                "{} has no producer and awaits none, with {} consumers",
                entry.key(),
                entry.consumers.len()
            );
            // An upstream SUBSCRIBE lasts only while someone it serves receives
            // the track or is about to.
            for (publisher, producer) in &entry.producers {
                if producer.state.origin != ProducerOrigin::Subscribe {
                    continue;
                }
                let served = match producer.state.peer {
                    SessionPeer::Client => entry.downstream_remains(),
                    SessionPeer::Relay => entry.client_remains(),
                };
                prop_assert!(
                    served,
                    "the upstream SUBSCRIBE to session {} for {} serves nobody",
                    publisher,
                    entry.key()
                );
            }
        }
        Ok(())
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_directory_keeps_its_bookkeeping_over_any_operation_sequence(
        operations in vec(operation(), 1..60)
    ) {
        // Arrange: a producer's idle ingest needs a runtime to be made
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let mut harness = Harness::new();

        for operation in &operations {
            // Act
            let silent_ends = harness.apply(operation)?;

            // Assert
            harness.check(&silent_ends)?;
        }
    }
}

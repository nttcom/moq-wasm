//! Drives the control plane as it runs in production — session workers,
//! sequences, directory and cache — with random sequences of the messages
//! client sessions send, and checks what each peer was sent. The relay's
//! requests to a peer wait for an answer the sequence gives later, so the
//! answers reach the relay in any order and other sessions' messages come in
//! between.
//!
//! A step lets the relay run until it can go on only with another message
//! or answer, then checks every peer's messages: each answer and
//! notification is one the peer's own messages call for, a subscriber of a
//! track is served by a publisher or told that the track ended, the relay
//! holds no upstream subscription nobody needs, and the routes and the
//! namespaces a peer was told of are the ones the sessions' requests give.
//! A relay that does not come to rest fails the step.
//!
//! The peers are clients of one relay and send no object, FETCH or update.
//! The route registry answers at once, and a peer sends only answers while
//! its SUBSCRIBE is unanswered, so the relay has handled every message sent.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::Arc,
};

use moqt::{RequestErrorCode, namespace::NamespaceInterest};
use proptest::{collection::vec, prelude::*, sample::Index};

use crate::modules::{
    domain::{
        session_id::SessionId,
        track_key::{TrackKey, namespace_has_prefix, namespace_prefixes_overlap},
    },
    test_support::{
        in_memory_route_registry::InMemoryRouteRegistry,
        running_session_workers::RunningSessionWorkers,
        scripted_session::{PublishAnswer, ScriptedPeer, Sent, SubscribeAnswer},
    },
};

const NAMESPACES: [&str; 2] = ["app/room", "app/lobby"];
/// Each track as the index of its namespace and its name.
const TRACKS: [(usize, &str); 3] = [(0, "video"), (0, "audio"), (1, "video")];
/// `app` covers both namespaces, so it overlaps the other prefixes. A client
/// is not authorized for the empty prefix.
const PREFIXES: [&str; 3] = ["app/room", "app/lobby", "app"];
const INTERESTS: [NamespaceInterest; 3] = [
    NamespaceInterest::Namespaces,
    NamespaceInterest::Tracks,
    NamespaceInterest::NamespacesAndTracks,
];
const SLOTS: usize = 3;
/// Answering every request can start more, as a SUBSCRIBE looks its track up
/// again; this many rounds answer them all.
const QUIET_ROUNDS: usize = 16;
/// Far more turns than the longest chain of tasks waking one another.
const SETTLE_TURNS: usize = 1_000;

fn track_key(track: usize) -> TrackKey {
    let (namespace, name) = TRACKS[track];
    TrackKey::new(NAMESPACES[namespace], name)
}

fn track_of(track_key: &TrackKey) -> usize {
    (0..TRACKS.len())
        .find(|track| self::track_key(*track) == *track_key)
        .unwrap_or_else(|| unreachable!("the peers name only TRACKS"))
}

fn namespace_of(track_namespace: &str) -> usize {
    NAMESPACES
        .iter()
        .position(|namespace| *namespace == track_namespace)
        .unwrap_or_else(|| unreachable!("the peers name only NAMESPACES"))
}

fn prefix_of(track_namespace_prefix: &str) -> usize {
    PREFIXES
        .iter()
        .position(|prefix| *prefix == track_namespace_prefix)
        .unwrap_or_else(|| unreachable!("the peers name only PREFIXES"))
}

fn interest_of(interest: NamespaceInterest) -> usize {
    INTERESTS
        .iter()
        .position(|listed| *listed == interest)
        .unwrap_or_else(|| unreachable!("INTERESTS lists every interest"))
}

struct Relay {
    workers: RunningSessionWorkers,
    route_registry: Arc<InMemoryRouteRegistry>,
}

impl Relay {
    fn start() -> Self {
        let route_registry = Arc::new(InMemoryRouteRegistry::default());
        Self {
            workers: RunningSessionWorkers::start_with_route_registry(route_registry.clone()),
            route_registry,
        }
    }

    async fn connect(&self, session_id: SessionId) -> ScriptedPeer {
        let (new_session, peer) = ScriptedPeer::new_session(session_id);
        self.workers.repo.lock().await.add(new_session);
        peer
    }
}

/// A runtime on which a run depends only on the messages and answers the
/// test gives: one thread, a paused clock and a fixed seed for `select!`.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .rng_seed(tokio::runtime::RngSeed::from_bytes(b"message sequences"))
        .build()
        .unwrap()
}

/// Lets every task run until none can go on without the test. The clock
/// stands still meanwhile, so no timeout ends a wait. Fails when the relay
/// is still running after `SETTLE_TURNS` turns of the scheduler: a task that
/// never comes to rest is waiting in a busy loop.
async fn settle() -> Result<(), TestCaseError> {
    let metrics = tokio::runtime::Handle::current().metrics();
    for _ in 0..SETTLE_TURNS {
        tokio::task::yield_now().await;
        if metrics.worker_local_queue_depth(0) == 0 {
            return Ok(());
        }
    }
    Err(TestCaseError::fail(
        "the relay keeps running without another message or answer",
    ))
}

#[derive(Clone, Debug)]
enum Operation {
    PublishNamespace {
        slot: usize,
        namespace: usize,
    },
    /// Ends one of the peer's PUBLISH_NAMESPACEs of the namespace, as a
    /// draft-18 peer does. With no `request` it ends all of them, as a draft-14
    /// peer does: its PUBLISH_NAMESPACE_DONE names only the namespace.
    PublishNamespaceDone {
        slot: usize,
        namespace: usize,
        request: Option<Index>,
    },
    SubscribeNamespace {
        slot: usize,
        prefix: usize,
        interest: usize,
    },
    UnsubscribeNamespace {
        pick: Index,
    },
    Publish {
        slot: usize,
        track: usize,
    },
    /// A peer ends an upstream subscription it serves: a PUBLISH of its own
    /// or a SUBSCRIBE of the relay it accepted.
    PublishDone {
        pick: Index,
    },
    Subscribe {
        slot: usize,
        track: usize,
    },
    /// A peer ends a subscription it receives: a SUBSCRIBE of its own or a
    /// PUBLISH of the relay it accepted.
    Unsubscribe {
        pick: Index,
    },
    AnswerSubscribe {
        pick: Index,
        accept: bool,
    },
    AnswerPublish {
        pick: Index,
        answer: PublishAnswer,
    },
    /// The peer leaves, and its slot connects again as a new session.
    Disconnect {
        slot: usize,
    },
}

fn operation() -> impl Strategy<Value = Operation> {
    let slot = || 0..SLOTS;
    let track = || 0..TRACKS.len();
    prop_oneof![
        3 => (slot(), 0..NAMESPACES.len())
            .prop_map(|(slot, namespace)| Operation::PublishNamespace { slot, namespace }),
        1 => (slot(), 0..NAMESPACES.len(), any::<Option<Index>>()).prop_map(
            |(slot, namespace, request)| Operation::PublishNamespaceDone { slot, namespace, request }
        ),
        2 => (slot(), 0..PREFIXES.len(), 0..INTERESTS.len()).prop_map(
            |(slot, prefix, interest)| Operation::SubscribeNamespace { slot, prefix, interest }
        ),
        1 => any::<Index>().prop_map(|pick| Operation::UnsubscribeNamespace { pick }),
        3 => (slot(), track()).prop_map(|(slot, track)| Operation::Publish { slot, track }),
        2 => any::<Index>().prop_map(|pick| Operation::PublishDone { pick }),
        5 => (slot(), track()).prop_map(|(slot, track)| Operation::Subscribe { slot, track }),
        2 => any::<Index>().prop_map(|pick| Operation::Unsubscribe { pick }),
        6 => (any::<Index>(), prop::bool::weighted(0.75))
            .prop_map(|(pick, accept)| Operation::AnswerSubscribe { pick, accept }),
        3 => (
            any::<Index>(),
            prop_oneof![
                3 => Just(PublishAnswer::Accept),
                1 => Just(PublishAnswer::Refuse),
                1 => Just(PublishAnswer::Blocked),
            ],
        )
            .prop_map(|(pick, answer)| Operation::AnswerPublish { pick, answer }),
        1 => slot().prop_map(|slot| Operation::Disconnect { slot }),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Live,
}

/// A subscription of a track between the relay and a peer, until it ends.
#[derive(Clone, Copy, Debug)]
struct Subscription {
    track: usize,
    state: State,
}

impl Subscription {
    fn pending(track: usize) -> Self {
        Self {
            track,
            state: State::Pending,
        }
    }
}

/// What one peer knows: its own messages and the relay's.
struct Slot {
    session_id: SessionId,
    peer: ScriptedPeer,
    /// How many of the relay's messages the peer has read.
    read: usize,
    /// The peer's PUBLISH_NAMESPACEs the relay accepted, by Request ID, each
    /// with its namespace. A peer may hold several of one namespace.
    namespaces: BTreeMap<u64, usize>,
    /// The peer's namespace subscriptions, as prefix and interest, once the
    /// relay accepted them.
    namespace_subscriptions: BTreeSet<(usize, usize)>,
    /// The namespaces the relay announced to the peer and has not withdrawn.
    announced: BTreeSet<usize>,
    /// The subscriptions the peer serves as a publisher, by Request ID: its
    /// PUBLISHes (even) and the relay's SUBSCRIBEs (odd).
    upstreams: BTreeMap<u64, Subscription>,
    /// The subscriptions the peer receives, by Request ID: its SUBSCRIBEs
    /// (even) and the relay's PUBLISHes (odd).
    downstreams: BTreeMap<u64, Subscription>,
    /// The peer disconnected; its slot is yet to connect again.
    left: bool,
}

impl Slot {
    fn new(session_id: SessionId, peer: ScriptedPeer) -> Self {
        Self {
            session_id,
            peer,
            read: 0,
            namespaces: BTreeMap::new(),
            namespace_subscriptions: BTreeSet::new(),
            announced: BTreeSet::new(),
            upstreams: BTreeMap::new(),
            downstreams: BTreeMap::new(),
            left: false,
        }
    }

    /// The peer's worker handles one message at a time, and only a SUBSCRIBE
    /// waits for another session; the peer sends only answers meanwhile, so
    /// that the relay has handled every message the peer sent.
    fn idle(&self) -> bool {
        !self.downstreams.iter().any(|(request_id, subscription)| {
            request_id % 2 == 0 && subscription.state == State::Pending
        })
    }

    fn serves(&self, track: usize) -> bool {
        self.upstreams
            .values()
            .any(|upstream| upstream.track == track && upstream.state == State::Live)
    }

    fn receives(&self, track: usize) -> bool {
        self.downstreams
            .values()
            .any(|downstream| downstream.track == track && downstream.state == State::Live)
    }

    fn owes_subscribe_answer(&self, track: usize) -> bool {
        self.upstreams.iter().any(|(request_id, upstream)| {
            request_id % 2 == 1 && upstream.track == track && upstream.state == State::Pending
        })
    }

    fn publishes_namespace(&self, namespace: usize) -> bool {
        self.namespaces.values().any(|held| *held == namespace)
    }

    fn publishes(&self, track: usize) -> bool {
        self.upstreams
            .iter()
            .any(|(request_id, upstream)| request_id % 2 == 0 && upstream.track == track)
    }

    /// The interest of the peer's subscription that is offered the track.
    fn track_subscription(&self, track: usize) -> Option<NamespaceInterest> {
        self.namespace_subscriptions
            .iter()
            .map(|(prefix, interest)| (PREFIXES[*prefix], INTERESTS[*interest]))
            .find(|(prefix, interest)| {
                interest.includes_tracks()
                    && namespace_has_prefix(NAMESPACES[TRACKS[track].0], prefix)
            })
            .map(|(_, interest)| interest)
    }

    fn is_told_of(&self, namespace: usize) -> bool {
        self.namespace_subscriptions
            .iter()
            .any(|(prefix, interest)| {
                INTERESTS[*interest].includes_namespaces()
                    && namespace_has_prefix(NAMESPACES[namespace], PREFIXES[*prefix])
            })
    }

    fn overlaps(&self, prefix: usize, interest: usize) -> bool {
        let wanted = INTERESTS[interest];
        self.namespace_subscriptions
            .iter()
            .any(|(held, held_interest)| {
                let held_interest = INTERESTS[*held_interest];
                namespace_prefixes_overlap(PREFIXES[*held], PREFIXES[prefix])
                    && ((wanted.includes_namespaces() && held_interest.includes_namespaces())
                        || (wanted.includes_tracks() && held_interest.includes_tracks()))
            })
    }
}

struct Harness {
    relay: Relay,
    slots: Vec<Slot>,
    next_session_id: SessionId,
}

impl Harness {
    async fn start() -> Self {
        let relay = Relay::start();
        let mut slots = Vec::new();
        for slot in 0..SLOTS {
            let session_id = slot as SessionId + 1;
            slots.push(Slot::new(session_id, relay.connect(session_id).await));
        }
        Self {
            relay,
            slots,
            next_session_id: SLOTS as SessionId + 1,
        }
    }

    fn apply(&mut self, operation: &Operation) {
        match *operation {
            Operation::PublishNamespace { slot, namespace } => {
                if self.slots[slot].idle() {
                    self.slots[slot]
                        .peer
                        .publish_namespace(NAMESPACES[namespace]);
                }
            }
            Operation::PublishNamespaceDone {
                slot,
                namespace,
                request,
            } => {
                let slot = &mut self.slots[slot];
                let requests: Vec<_> = slot
                    .namespaces
                    .iter()
                    .filter(|(_, held)| **held == namespace)
                    .map(|(request_id, _)| *request_id)
                    .collect();
                if !slot.idle() || requests.is_empty() {
                    return;
                }
                let ended = request.map(|pick| requests[pick.index(requests.len())]);
                slot.namespaces.retain(|request_id, held| {
                    *held != namespace || ended.is_some_and(|ended| ended != *request_id)
                });
                slot.peer
                    .publish_namespace_done(NAMESPACES[namespace], ended);
            }
            Operation::SubscribeNamespace {
                slot,
                prefix,
                interest,
            } => {
                if self.slots[slot].idle() {
                    self.slots[slot]
                        .peer
                        .subscribe_namespace(PREFIXES[prefix], INTERESTS[interest]);
                }
            }
            Operation::UnsubscribeNamespace { pick } => {
                let held: Vec<_> = (0..SLOTS)
                    .filter(|slot| self.slots[*slot].idle())
                    .flat_map(|slot| {
                        self.slots[slot]
                            .namespace_subscriptions
                            .iter()
                            .map(move |(prefix, interest)| (slot, *prefix, *interest))
                    })
                    .collect();
                if held.is_empty() {
                    return;
                }
                let (slot, prefix, interest) = held[pick.index(held.len())];
                let slot = &mut self.slots[slot];
                slot.namespace_subscriptions.remove(&(prefix, interest));
                // The namespaces announced under a subscription end with it.
                if INTERESTS[interest].includes_namespaces() {
                    slot.announced.retain(|namespace| {
                        !namespace_has_prefix(NAMESPACES[*namespace], PREFIXES[prefix])
                    });
                }
                slot.peer
                    .unsubscribe_namespace(PREFIXES[prefix], INTERESTS[interest]);
            }
            Operation::Publish { slot, track } => {
                if self.slots[slot].idle() && !self.slots[slot].publishes(track) {
                    let slot = &mut self.slots[slot];
                    let request_id = slot.peer.publish(&track_key(track));
                    slot.upstreams
                        .insert(request_id, Subscription::pending(track));
                }
            }
            Operation::PublishDone { pick } => {
                let served = self.picked(pick, |slot| &slot.upstreams);
                if let Some((slot, request_id)) = served {
                    let slot = &mut self.slots[slot];
                    slot.upstreams.remove(&request_id);
                    slot.peer.publish_done(request_id);
                }
            }
            Operation::Subscribe { slot, track } => {
                if self.slots[slot].idle() {
                    let slot = &mut self.slots[slot];
                    let request_id = slot.peer.subscribe(&track_key(track));
                    slot.downstreams
                        .insert(request_id, Subscription::pending(track));
                }
            }
            Operation::Unsubscribe { pick } => {
                let received = self.picked(pick, |slot| &slot.downstreams);
                if let Some((slot, request_id)) = received {
                    let slot = &mut self.slots[slot];
                    slot.downstreams.remove(&request_id);
                    slot.peer.unsubscribe(request_id);
                }
            }
            Operation::AnswerSubscribe { pick, accept } => {
                let Some((slot, request_id)) = self.unanswered(pick, |slot| &slot.upstreams) else {
                    return;
                };
                let slot = &mut self.slots[slot];
                if accept {
                    if let Some(upstream) = slot.upstreams.get_mut(&request_id) {
                        upstream.state = State::Live;
                    }
                    slot.peer
                        .answer_subscribe(request_id, SubscribeAnswer::Accept);
                } else {
                    slot.upstreams.remove(&request_id);
                    slot.peer
                        .answer_subscribe(request_id, SubscribeAnswer::Refuse);
                }
            }
            Operation::AnswerPublish { pick, answer } => {
                let Some((slot, request_id)) = self.unanswered(pick, |slot| &slot.downstreams)
                else {
                    return;
                };
                let slot = &mut self.slots[slot];
                // PUBLISH_BLOCKED answers a draft-18 SUBSCRIBE_TRACKS only.
                let blocks = slot.downstreams.get(&request_id).is_some_and(|downstream| {
                    slot.track_subscription(downstream.track) == Some(NamespaceInterest::Tracks)
                });
                let answer = match answer {
                    PublishAnswer::Blocked if !blocks => PublishAnswer::Refuse,
                    answer => answer,
                };
                match answer {
                    PublishAnswer::Accept => {
                        if let Some(downstream) = slot.downstreams.get_mut(&request_id) {
                            downstream.state = State::Live;
                        }
                    }
                    // moqt refuses later PUBLISHes of a PUBLISH_BLOCKED track
                    // while its SUBSCRIBE_TRACKS lives (draft-18 §6.1).
                    PublishAnswer::Refuse | PublishAnswer::Blocked => {
                        slot.downstreams.remove(&request_id);
                    }
                }
                slot.peer.answer_publish(request_id, answer);
            }
            Operation::Disconnect { slot } => {
                if self.slots[slot].idle() {
                    let left = &mut self.slots[slot];
                    left.peer.disconnect();
                    *left = Slot {
                        left: true,
                        ..Slot::new(left.session_id, left.peer.clone())
                    };
                }
            }
        }
    }

    /// A live subscription of an idle slot, picked from the map `of` gives.
    fn picked(
        &self,
        pick: Index,
        of: impl Fn(&Slot) -> &BTreeMap<u64, Subscription>,
    ) -> Option<(usize, u64)> {
        let live: Vec<_> = (0..SLOTS)
            .filter(|slot| self.slots[*slot].idle())
            .flat_map(|slot| {
                of(&self.slots[slot])
                    .iter()
                    .filter(|(_, subscription)| subscription.state == State::Live)
                    .map(move |(request_id, _)| (slot, *request_id))
            })
            .collect();
        (!live.is_empty()).then(|| live[pick.index(live.len())])
    }

    /// A request of the relay a peer has yet to answer, picked from the map
    /// `of` gives.
    fn unanswered(
        &self,
        pick: Index,
        of: impl Fn(&Slot) -> &BTreeMap<u64, Subscription>,
    ) -> Option<(usize, u64)> {
        let pending: Vec<_> = (0..SLOTS)
            .flat_map(|slot| {
                of(&self.slots[slot])
                    .iter()
                    .filter(|(request_id, subscription)| {
                        *request_id % 2 == 1 && subscription.state == State::Pending
                    })
                    .map(move |(request_id, _)| (slot, *request_id))
            })
            .collect();
        (!pending.is_empty()).then(|| pending[pick.index(pending.len())])
    }

    /// Each peer reads what the relay sent it since the last step.
    fn read(&mut self) -> Result<(), TestCaseError> {
        for slot in 0..SLOTS {
            if self.slots[slot].left {
                continue;
            }
            let sent = self.slots[slot].peer.sent();
            for message in &sent[self.slots[slot].read..] {
                self.read_message(slot, message)?;
            }
            self.slots[slot].read = sent.len();
        }
        Ok(())
    }

    fn read_message(&mut self, slot: usize, message: &Sent) -> Result<(), TestCaseError> {
        let session_id = self.slots[slot].session_id;
        match message {
            Sent::PublishNamespaceOk {
                request_id,
                track_namespace,
            } => {
                self.slots[slot]
                    .namespaces
                    .insert(*request_id, namespace_of(track_namespace));
            }
            Sent::PublishNamespaceError(track_namespace) => {
                prop_assert!(
                    false,
                    "session {session_id} was refused PUBLISH_NAMESPACE {track_namespace}"
                );
            }
            Sent::SubscribeNamespaceOk { prefix, interest } => {
                let (prefix, interest) = (prefix_of(prefix), interest_of(*interest));
                prop_assert!(
                    !self.slots[slot].overlaps(prefix, interest),
                    "session {session_id} was accepted a namespace subscription overlapping one it holds"
                );
                self.slots[slot]
                    .namespace_subscriptions
                    .insert((prefix, interest));
            }
            Sent::SubscribeNamespaceError { prefix, interest } => {
                prop_assert!(
                    self.slots[slot].overlaps(prefix_of(prefix), interest_of(*interest)),
                    "session {session_id} was refused a namespace subscription to {prefix:?} that overlaps none it holds"
                );
            }
            Sent::PublishOk { request_id } => {
                let upstream = self.slots[slot].upstreams.get_mut(request_id);
                prop_assert!(
                    upstream
                        .as_ref()
                        .is_some_and(|upstream| upstream.state == State::Pending),
                    "session {session_id} got PUBLISH_OK for {request_id}, which awaits none"
                );
                if let Some(upstream) = upstream {
                    upstream.state = State::Live;
                }
            }
            Sent::PublishError { request_id } => {
                prop_assert!(
                    false,
                    "session {session_id} was refused PUBLISH {request_id}"
                );
            }
            Sent::SubscribeOk { request_id } => {
                let downstream = self.slots[slot].downstreams.get_mut(request_id);
                prop_assert!(
                    downstream
                        .as_ref()
                        .is_some_and(|downstream| downstream.state == State::Pending),
                    "session {session_id} got SUBSCRIBE_OK for {request_id}, which awaits none"
                );
                if let Some(downstream) = downstream {
                    downstream.state = State::Live;
                }
            }
            Sent::SubscribeError { request_id, code } => {
                let refused = self.slots[slot].downstreams.remove(request_id);
                prop_assert!(
                    refused.is_some_and(|refused| refused.state == State::Pending),
                    "session {session_id} got SUBSCRIBE_ERROR for {request_id}, which awaits none"
                );
                if let Some(refused) = refused {
                    prop_assert!(
                        !self.slots.iter().any(|slot| slot.serves(refused.track)),
                        "session {session_id} was refused {} with {code:?} while a publisher serves it",
                        track_key(refused.track)
                    );
                }
            }
            Sent::Subscribe {
                request_id,
                track_key,
            } => {
                let track = track_of(track_key);
                let slot = &mut self.slots[slot];
                prop_assert!(
                    slot.publishes_namespace(TRACKS[track].0) || slot.publishes(track),
                    "session {session_id} got SUBSCRIBE for {track_key}, which it does not publish"
                );
                prop_assert!(
                    !slot.serves(track),
                    "session {session_id} got a second SUBSCRIBE for {track_key}, which it serves"
                );
                slot.upstreams
                    .insert(*request_id, Subscription::pending(track));
            }
            Sent::Unsubscribe { request_id } => {
                let ended = self.slots[slot].upstreams.remove(request_id);
                prop_assert!(
                    ended.is_some_and(|ended| ended.state == State::Live),
                    "session {session_id} got UNSUBSCRIBE for {request_id}, which it does not serve"
                );
            }
            Sent::Publish {
                request_id,
                track_key,
            } => {
                let track = track_of(track_key);
                let slot = &mut self.slots[slot];
                let interest = slot.track_subscription(track);
                prop_assert!(
                    interest.is_some(),
                    "session {session_id} got PUBLISH for {track_key}, which it subscribes no tracks of"
                );
                prop_assert!(
                    interest != Some(NamespaceInterest::Tracks) || !slot.publishes(track),
                    "session {session_id} got PUBLISH for {track_key}, which it publishes itself"
                );
                // A PUBLISH the peer has yet to answer may be one of a track
                // that ended since, so it bars no PUBLISH of the track.
                prop_assert!(
                    !slot.receives(track),
                    "session {session_id} got PUBLISH for {track_key}, which it already receives"
                );
                slot.downstreams
                    .insert(*request_id, Subscription::pending(track));
            }
            Sent::PublishDone { request_id } => {
                let ended = self.slots[slot].downstreams.remove(request_id);
                prop_assert!(
                    ended.is_some_and(|ended| ended.state == State::Live),
                    "session {session_id} got PUBLISH_DONE for {request_id}, which it does not receive"
                );
            }
            Sent::PublishNamespace(track_namespace) => {
                let namespace = namespace_of(track_namespace);
                prop_assert!(
                    self.slots[slot].announced.insert(namespace),
                    "session {session_id} was announced {track_namespace} twice"
                );
            }
            Sent::PublishNamespaceDone(track_namespace) => {
                prop_assert!(
                    self.slots[slot]
                        .announced
                        .remove(&namespace_of(track_namespace)),
                    "session {session_id} was told {track_namespace} is withdrawn, which it was not announced"
                );
            }
            Sent::Close(code) => {
                prop_assert!(false, "session {session_id} was closed with {code:?}");
            }
        }
        Ok(())
    }

    /// What holds whenever the relay can go on only with another message or
    /// answer.
    fn check(&self) -> Result<(), TestCaseError> {
        for track in 0..TRACKS.len() {
            let track_key = track_key(track);
            let awaited = self
                .slots
                .iter()
                .any(|slot| slot.owes_subscribe_answer(track));
            for slot in &self.slots {
                let session_id = slot.session_id;
                let waits = slot.downstreams.iter().any(|(request_id, downstream)| {
                    request_id % 2 == 0
                        && downstream.track == track
                        && downstream.state == State::Pending
                });
                prop_assert!(
                    !waits || awaited,
                    "session {session_id} waits for the answer to its SUBSCRIBE of {track_key}, and no publisher is asked for it"
                );
                if slot.owes_subscribe_answer(track) {
                    continue;
                }
                let served: Vec<_> = slot
                    .upstreams
                    .iter()
                    .filter(|(_, upstream)| {
                        upstream.track == track && upstream.state == State::Live
                    })
                    .map(|(request_id, _)| *request_id)
                    .collect();
                prop_assert!(
                    served.len() <= 1,
                    "session {session_id} serves {track_key} on the requests {served:?} at once"
                );
            }
            if awaited {
                continue;
            }
            let served = self.slots.iter().any(|slot| slot.serves(track));
            let received = self.slots.iter().any(|slot| slot.receives(track));
            prop_assert!(
                served || !received,
                "a subscriber of {track_key} is neither served by a publisher nor told it ended"
            );
            let subscribed = self.slots.iter().find(|slot| {
                slot.upstreams.iter().any(|(request_id, upstream)| {
                    request_id % 2 == 1 && upstream.track == track && upstream.state == State::Live
                })
            });
            prop_assert!(
                received || subscribed.is_none(),
                "the relay keeps its SUBSCRIBE of {track_key} to session {:?}, which no subscriber needs",
                subscribed.map(|slot| slot.session_id)
            );
        }

        let routes = self.relay.route_registry.routes();
        let namespace_publishers: BTreeSet<_> = self
            .slots
            .iter()
            .flat_map(|slot| slot.namespaces.values())
            .map(|namespace| NAMESPACES[*namespace].to_string())
            .collect();
        prop_assert_eq!(
            routes.namespace_publishers,
            namespace_publishers,
            "namespace publisher routes"
        );
        let namespace_subscribers: BTreeSet<_> = self
            .slots
            .iter()
            .flat_map(|slot| slot.namespace_subscriptions.iter())
            .map(|(prefix, _)| PREFIXES[*prefix].to_string())
            .collect();
        prop_assert_eq!(
            routes.namespace_subscribers,
            namespace_subscribers,
            "namespace subscriber routes"
        );
        let track_publishers: HashSet<_> = self
            .slots
            .iter()
            .flat_map(|slot| slot.upstreams.iter())
            .filter(|(request_id, upstream)| *request_id % 2 == 0 && upstream.state == State::Live)
            .map(|(_, upstream)| track_key(upstream.track))
            .collect();
        prop_assert_eq!(routes.track_publishers, track_publishers, "track routes");

        let published: BTreeSet<_> = self
            .slots
            .iter()
            .flat_map(|slot| slot.namespaces.values().copied())
            .collect();
        for slot in &self.slots {
            let expected: BTreeSet<_> = published
                .iter()
                .copied()
                .filter(|namespace| slot.is_told_of(*namespace))
                .collect();
            prop_assert_eq!(
                &slot.announced,
                &expected,
                "the namespaces session {} was announced",
                slot.session_id
            );
        }
        Ok(())
    }

    async fn reconnect(&mut self, slot: usize) -> Result<(), TestCaseError> {
        let session_id = self.slots[slot].session_id;
        let directory = &self.relay.workers.local_pub_sub_directory;
        prop_assert!(
            self.relay
                .workers
                .repo
                .lock()
                .await
                .session_states()
                .iter()
                .all(|session| session.session_id != session_id),
            "session {session_id} is still in the repository after it disconnected"
        );
        prop_assert!(
            directory.tracks_fed_by(session_id).is_empty(),
            "session {session_id} still feeds a track after it disconnected"
        );
        prop_assert!(
            directory
                .downstream_subscription_states()
                .iter()
                .all(|subscription| subscription.subscriber_session_id != session_id),
            "session {session_id} still receives a track after it disconnected"
        );
        let session_id = self.next_session_id;
        self.next_session_id += 1;
        self.slots[slot] = Slot::new(session_id, self.relay.connect(session_id).await);
        Ok(())
    }

    async fn step(&mut self, operation: &Operation) -> Result<(), TestCaseError> {
        self.apply(operation);
        settle().await?;
        self.read()?;
        for slot in 0..SLOTS {
            if self.slots[slot].left {
                self.reconnect(slot).await?;
            }
        }
        self.check()
    }

    /// Every peer accepts what the relay still asks of it, until the relay
    /// asks nothing more; then every peer leaves.
    async fn finish(&mut self) -> Result<(), TestCaseError> {
        for _ in 0..QUIET_ROUNDS {
            let mut answered = false;
            for slot in &mut self.slots {
                for request_id in slot.peer.pending_subscribes() {
                    if let Some(upstream) = slot.upstreams.get_mut(&request_id) {
                        upstream.state = State::Live;
                    }
                    slot.peer
                        .answer_subscribe(request_id, SubscribeAnswer::Accept);
                    answered = true;
                }
                for request_id in slot.peer.pending_publishes() {
                    if let Some(downstream) = slot.downstreams.get_mut(&request_id) {
                        downstream.state = State::Live;
                    }
                    slot.peer.answer_publish(request_id, PublishAnswer::Accept);
                    answered = true;
                }
            }
            if !answered {
                break;
            }
            settle().await?;
            self.read()?;
            self.check()?;
        }
        prop_assert!(
            self.slots
                .iter()
                .all(|slot| slot.peer.pending_subscribes().is_empty()
                    && slot.peer.pending_publishes().is_empty()),
            "the relay keeps asking after {QUIET_ROUNDS} rounds of answers"
        );

        for slot in &self.slots {
            slot.peer.disconnect();
        }
        settle().await?;
        let directory = &self.relay.workers.local_pub_sub_directory;
        prop_assert!(
            directory.upstream_tracks.is_empty(),
            "a track is left after every session disconnected"
        );
        prop_assert!(
            directory.publisher_namespaces.is_empty(),
            "a published namespace is left after every session disconnected"
        );
        prop_assert!(
            directory.subscriber_namespaces.is_empty(),
            "a namespace subscription is left after every session disconnected"
        );
        prop_assert!(
            self.relay
                .workers
                .repo
                .lock()
                .await
                .session_states()
                .is_empty(),
            "a session is left in the repository after every session disconnected"
        );
        let routes = self.relay.route_registry.routes();
        prop_assert!(
            routes.namespace_publishers.is_empty()
                && routes.namespace_subscribers.is_empty()
                && routes.track_publishers.is_empty(),
            "a route is left after every session disconnected: {routes:?}"
        );
        Ok(())
    }
}

async fn run(operations: Vec<Operation>) -> Result<(), TestCaseError> {
    // Arrange
    let mut harness = Harness::start().await;

    for operation in &operations {
        // Act / Assert
        harness.step(operation).await?;
    }
    harness.finish().await
}

proptest! {
    #[test]
    fn every_peer_is_sent_what_the_sessions_messages_call_for(
        operations in vec(operation(), 1..40)
    ) {
        runtime().block_on(run(operations))?;
    }
}

#[test]
fn a_subscribe_of_a_track_whose_last_publisher_left_waits_for_the_publisher_still_asked() {
    runtime().block_on(async {
        // Arrange: the relay asks a second publisher for a track it serves,
        // whose first publisher then ends it
        let relay = Relay::start();
        let first_publisher = relay.connect(1).await;
        let second_publisher = relay.connect(2).await;
        let subscriber = relay.connect(3).await;
        let late_subscriber = relay.connect(4).await;
        let track_key = TrackKey::new("room", "video");
        let publish = first_publisher.publish(&track_key);
        settle().await.unwrap();
        subscriber.subscribe(&track_key);
        settle().await.unwrap();
        second_publisher.publish_namespace("room");
        settle().await.unwrap();
        first_publisher.publish_done(publish);
        settle().await.unwrap();

        // Act
        let request_id = late_subscriber.subscribe(&track_key);
        let waits = settle().await;
        let unanswered = late_subscriber.sent();
        let upstream_subscribes = second_publisher.pending_subscribes();
        second_publisher.answer_subscribe(upstream_subscribes[0], SubscribeAnswer::Accept);
        settle().await.unwrap();

        // Assert: the SUBSCRIBE waits at rest, and is answered with the publisher's answer
        assert!(waits.is_ok());
        assert!(unanswered.is_empty());
        assert!(
            late_subscriber
                .sent()
                .contains(&Sent::SubscribeOk { request_id })
        );
    });
}

#[test]
fn a_subscriber_is_answered_once_a_publisher_of_the_namespace_accepts_the_subscribe() {
    runtime().block_on(async {
        // Arrange
        let relay = Relay::start();
        let publisher = relay.connect(1).await;
        let subscriber = relay.connect(2).await;
        publisher.publish_namespace("room");
        settle().await.unwrap();
        let request_id = subscriber.subscribe(&TrackKey::new("room", "video"));
        settle().await.unwrap();
        let upstream_subscribes = publisher.pending_subscribes();

        // Act
        publisher.answer_subscribe(upstream_subscribes[0], SubscribeAnswer::Accept);
        settle().await.unwrap();

        // Assert
        assert_eq!(upstream_subscribes.len(), 1);
        assert!(
            subscriber
                .sent()
                .contains(&Sent::SubscribeOk { request_id })
        );
    });
}

#[test]
fn a_namespace_published_again_while_its_route_is_withdrawn_keeps_its_route() {
    runtime().block_on(async {
        // Arrange: the first publisher's withdrawal of the route is in flight
        let relay = Relay::start();
        let first_publisher = relay.connect(1).await;
        let second_publisher = relay.connect(2).await;
        first_publisher.publish_namespace("room");
        settle().await.unwrap();
        let withdrawal = relay.route_registry.hold_next_route_call();
        first_publisher.publish_namespace_done("room", None);
        settle().await.unwrap();

        // Act
        second_publisher.publish_namespace("room");
        settle().await.unwrap();
        withdrawal.notify_one();
        settle().await.unwrap();

        // Assert
        assert_eq!(
            second_publisher.sent(),
            vec![Sent::PublishNamespaceOk {
                request_id: 0,
                track_namespace: "room".to_string(),
            }]
        );
        assert_eq!(
            relay.route_registry.routes().namespace_publishers,
            BTreeSet::from(["room".to_string()])
        );
    });
}

#[test]
fn a_publish_namespace_whose_route_is_not_registered_is_refused_and_withdrawn() {
    runtime().block_on(async {
        // Arrange
        let relay = Relay::start();
        let publisher = relay.connect(1).await;
        relay.route_registry.fail_next_namespace_registration();

        // Act
        publisher.publish_namespace("room");
        settle().await.unwrap();

        // Assert
        assert_eq!(
            publisher.sent(),
            vec![Sent::PublishNamespaceError("room".to_string())]
        );
        assert!(
            !relay
                .workers
                .local_pub_sub_directory
                .has_namespace_publishers("room")
        );
        assert!(
            relay
                .route_registry
                .routes()
                .namespace_publishers
                .is_empty()
        );
    });
}

#[test]
fn a_prefix_subscribed_while_its_last_subscriber_is_cleaned_up_is_accepted() {
    runtime().block_on(async {
        // Arrange: the withdrawal of the route of the disconnected subscriber is in flight
        let relay = Relay::start();
        let leaving_subscriber = relay.connect(1).await;
        let subscriber = relay.connect(2).await;
        leaving_subscriber.subscribe_namespace("room", NamespaceInterest::Namespaces);
        settle().await.unwrap();
        let withdrawal = relay.route_registry.hold_next_route_call();
        leaving_subscriber.disconnect();
        settle().await.unwrap();

        // Act
        subscriber.subscribe_namespace("room", NamespaceInterest::Namespaces);
        settle().await.unwrap();
        withdrawal.notify_one();
        settle().await.unwrap();

        // Assert
        assert_eq!(
            subscriber.sent(),
            vec![Sent::SubscribeNamespaceOk {
                prefix: "room".to_string(),
                interest: NamespaceInterest::Namespaces,
            }]
        );
        assert_eq!(
            relay.route_registry.routes().namespace_subscribers,
            BTreeSet::from(["room".to_string()])
        );
    });
}

#[test]
fn a_prefix_subscriber_joining_a_failed_registration_registers_the_route() {
    runtime().block_on(async {
        // Arrange: the first subscriber's registration of the route is in flight and fails
        let relay = Relay::start();
        let first_subscriber = relay.connect(1).await;
        let second_subscriber = relay.connect(2).await;
        let registration = relay.route_registry.hold_next_route_call();
        relay.route_registry.fail_next_namespace_registration();
        first_subscriber.subscribe_namespace("room", NamespaceInterest::Namespaces);
        settle().await.unwrap();

        // Act
        second_subscriber.subscribe_namespace("room", NamespaceInterest::Namespaces);
        settle().await.unwrap();
        registration.notify_one();
        settle().await.unwrap();

        // Assert
        assert_eq!(
            first_subscriber.sent(),
            vec![Sent::SubscribeNamespaceError {
                prefix: "room".to_string(),
                interest: NamespaceInterest::Namespaces,
            }]
        );
        assert_eq!(
            second_subscriber.sent(),
            vec![Sent::SubscribeNamespaceOk {
                prefix: "room".to_string(),
                interest: NamespaceInterest::Namespaces,
            }]
        );
        assert_eq!(
            relay.route_registry.routes().namespace_subscribers,
            BTreeSet::from(["room".to_string()])
        );
    });
}

#[test]
fn a_namespace_announced_while_its_route_fails_to_register_is_withdrawn() {
    runtime().block_on(async {
        // Arrange: a namespace subscriber is told of the namespace while its route is registering
        let relay = Relay::start();
        let publisher = relay.connect(1).await;
        let subscriber = relay.connect(2).await;
        let registration = relay.route_registry.hold_next_route_call();
        publisher.publish_namespace("room");
        settle().await.unwrap();
        subscriber.subscribe_namespace("room", NamespaceInterest::Namespaces);
        settle().await.unwrap();
        relay.route_registry.fail_next_namespace_registration();

        // Act
        registration.notify_one();
        settle().await.unwrap();

        // Assert
        assert_eq!(
            subscriber.sent(),
            vec![
                Sent::SubscribeNamespaceOk {
                    prefix: "room".to_string(),
                    interest: NamespaceInterest::Namespaces,
                },
                Sent::PublishNamespace("room".to_string()),
                Sent::PublishNamespaceDone("room".to_string()),
            ]
        );
    });
}

#[test]
fn a_subscribe_whose_publisher_ends_the_upstream_subscription_as_it_accepts_is_refused() {
    runtime().block_on(async {
        // Arrange
        let relay = Relay::start();
        let publisher = relay.connect(1).await;
        let subscriber = relay.connect(2).await;
        publisher.publish_namespace("room");
        settle().await.unwrap();
        let request_id = subscriber.subscribe(&TrackKey::new("room", "video"));
        settle().await.unwrap();
        let upstream_subscribe = publisher.pending_subscribes()[0];

        // Act: the relay handles the PUBLISH_DONE before the SUBSCRIBE_OK
        publisher.publish_done(upstream_subscribe);
        publisher.answer_subscribe(upstream_subscribe, SubscribeAnswer::Accept);
        settle().await.unwrap();

        // Assert
        assert_eq!(
            subscriber.sent(),
            vec![Sent::SubscribeError {
                request_id,
                code: RequestErrorCode::TrackDoesNotExist,
            }]
        );
    });
}

#[test]
fn an_unsubscribe_leaves_the_other_subscribers_waiting_for_the_publisher_still_asked() {
    runtime().block_on(async {
        // Arrange: the track's only publisher sends PUBLISH_DONE while the relay's SUBSCRIBE to a second publisher is unanswered
        let relay = Relay::start();
        let first_publisher = relay.connect(1).await;
        let second_publisher = relay.connect(2).await;
        let leaving_subscriber = relay.connect(3).await;
        let staying_subscriber = relay.connect(4).await;
        let track_key = TrackKey::new("room", "video");
        let publish = first_publisher.publish(&track_key);
        settle().await.unwrap();
        let leaving_request_id = leaving_subscriber.subscribe(&track_key);
        let staying_request_id = staying_subscriber.subscribe(&track_key);
        settle().await.unwrap();
        second_publisher.publish_namespace("room");
        settle().await.unwrap();
        first_publisher.publish_done(publish);
        settle().await.unwrap();

        // Act
        leaving_subscriber.unsubscribe(leaving_request_id);
        settle().await.unwrap();
        let upstream_subscribes = second_publisher.pending_subscribes();
        second_publisher.answer_subscribe(upstream_subscribes[0], SubscribeAnswer::Accept);
        settle().await.unwrap();

        // Assert: the staying subscriber is not sent PUBLISH_DONE and the second publisher is not unsubscribed
        assert_eq!(
            staying_subscriber.sent(),
            vec![Sent::SubscribeOk {
                request_id: staying_request_id,
            }]
        );
        assert!(
            second_publisher
                .sent()
                .iter()
                .all(|sent| !matches!(sent, Sent::Unsubscribe { .. }))
        );
    });
}

#[test]
fn a_track_published_while_its_last_publisher_withdraws_the_route_keeps_its_route() {
    runtime().block_on(async {
        // Arrange: the first publisher's withdrawal of the route is in flight
        let relay = Relay::start();
        let first_publisher = relay.connect(1).await;
        let second_publisher = relay.connect(2).await;
        let track_key = TrackKey::new("room", "video");
        let publish = first_publisher.publish(&track_key);
        settle().await.unwrap();
        let withdrawal = relay.route_registry.hold_next_route_call();
        first_publisher.publish_done(publish);
        settle().await.unwrap();
        let request_id = second_publisher.publish(&track_key);
        settle().await.unwrap();

        // Act
        withdrawal.notify_one();
        settle().await.unwrap();

        // Assert
        assert!(
            second_publisher
                .sent()
                .contains(&Sent::PublishOk { request_id })
        );
        assert!(
            relay
                .route_registry
                .routes()
                .track_publishers
                .contains(&track_key)
        );
    });
}

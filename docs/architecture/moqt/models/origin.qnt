// A model of one track of `Origin` (crates/moqt/src/modules/moqt/domains/origin.rs).
//
// Each action is one critical section of the implementation: what runs while
// `state` (and, where noted, `publishing`) is locked. A lock released between
// two steps of a call is a point where another action may interleave.
//
// `origin_current` is the implementation. Each other instance turns off one
// switch the implementation relies on, and fails the invariant it guards:
// - CHECK_ENDED: a SUBSCRIBE consults `lifecycle` (`endedTrackTakesNoSubscribe`).
// - ATOMIC_HANDLE: `accept_subscribe` checks the track under the lock it
//   registers the outlet with, instead of `handle` checking it under a lock
//   of its own first (`originAnswersEverySubscribe`).
//
// Simulate: npx @informalsystems/quint run --main origin_current --invariant safety origin.qnt
// Check every run up to 7 steps (Apalache, Java 21+):
//   npx @informalsystems/quint verify --main origin_current --invariant safety --max-steps 7 origin.qnt
module origin {
  // Subscribers, each with at most one SUBSCRIBE in flight.
  const SUBSCRIBERS: Set[str]
  // Sessions `Origin::publish` sends PUBLISH on, one PUBLISH at a time each.
  const PUBLISH_PEERS: Set[str]
  const CHECK_ENDED: bool
  const ATOMIC_HANDLE: bool

  // An outlet's `OutletState`: `Serving` with its `ServedTrack`, or `Released`.
  type Publication = Subscribed | Published | Released

  // A SUBSCRIBE as its sender and the origin's handler see it.
  type SubscribeState =
    | SIdle          // not sent
    | SSent          // SUBSCRIBE in flight
    | SChecked       // `handle` found the track live and unlocked `state`
    | SActive        // SUBSCRIBE_OK received
    | SUnsubscribing // UNSUBSCRIBE in flight
    | SUnsubscribed
    | SDone          // PUBLISH_DONE received
    | SRejected      // TRACK_DOES_NOT_EXIST received
    | SNotSupported  // the handler was dropped and its guard answered NOT_SUPPORTED

  // A PUBLISH the origin sent, as its peer sees it.
  type PeerState =
    | PIdle
    | POffered       // PUBLISH in flight, `publishing` held
    | PActive        // PUBLISH_OK sent
    | PUnsubscribing
    | PUnsubscribed
    | PDone
    | PRefused       // PUBLISH_ERROR sent

  type State = {
    registered: bool,
    ended: bool,
    outlets: str -> Publication,
    // `OriginState::subscriptions`
    subscriptions: Set[str],
    // The peer whose `publish` holds `publishing`, or "".
    publishing: str,
    // Peers whose PUBLISH_OK arrived and that `publish` has not registered yet.
    registering: Set[str],
    subscribers: str -> SubscribeState,
    peers: str -> PeerState,
  }

  var s: State

  pure val NOBODY = ""

  def live: bool = s.registered and not(CHECK_ENDED and s.ended)

  action init = s' = {
    registered: false,
    ended: false,
    outlets: Map(),
    subscriptions: Set(),
    publishing: NOBODY,
    registering: Set(),
    subscribers: SUBSCRIBERS.mapBy(_ => SIdle),
    peers: PUBLISH_PEERS.mapBy(_ => PIdle),
  }

  // `ensure_track`
  action ensureTrack = s' = { ...s, registered: true, ended: false }

  // `end_track`: withdraws a PUBLISH in flight (`withdrawPublish`), waits for
  // `publishing`, takes every outlet and sends PUBLISH_DONE on the serving
  // ones.
  action endTrack = all {
    s.registered,
    s.publishing == NOBODY,
    s' = {
      ...s,
      ended: true,
      outlets: Map(),
      subscriptions: Set(),
      subscribers: s.subscribers.keys().mapBy(r =>
        if (s.outlets.keys().contains(r) and s.outlets.get(r) == Subscribed) SDone
        else s.subscribers.get(r)),
      peers: s.peers.keys().mapBy(p =>
        if (s.outlets.keys().contains(p) and s.outlets.get(p) == Published) PDone
        else s.peers.get(p)),
    },
  }

  action sendSubscribe(r: str): bool = all {
    s.subscribers.get(r) == SIdle,
    s' = { ...s, subscribers: s.subscribers.set(r, SSent) },
  }

  // `accept_subscribe`, holding `state` across SUBSCRIBE_OK.
  def accepted(r: str): State = {
    ...s,
    outlets: s.outlets.put(r, Subscribed),
    subscriptions: s.subscriptions.union(Set(r)),
    subscribers: s.subscribers.set(r, SActive),
  }

  // `handle(SessionEvent::Subscribe)`. When the track is not live, the
  // SUBSCRIBE goes back to the caller, which answers TRACK_DOES_NOT_EXIST.
  action handleSubscribe(r: str): bool = all {
    s.subscribers.get(r) == SSent,
    if (not(live)) s' = { ...s, subscribers: s.subscribers.set(r, SRejected) }
    else if (ATOMIC_HANDLE) s' = accepted(r)
    else s' = { ...s, subscribers: s.subscribers.set(r, SChecked) },
  }

  // `accept_subscribe` after `handle` checked the track under a lock of its
  // own: it fails when the track is no longer live, and the dropped handler
  // answers NOT_SUPPORTED.
  action acceptSubscribe(r: str): bool = all {
    s.subscribers.get(r) == SChecked,
    if (live) s' = accepted(r)
    else s' = { ...s, subscribers: s.subscribers.set(r, SNotSupported) },
  }

  action sendUnsubscribe(r: str): bool = all {
    s.subscribers.get(r) == SActive,
    s' = { ...s, subscribers: s.subscribers.set(r, SUnsubscribing) },
  }

  // `release` through `lock_state_knowing`: waits for `publishing` unless it
  // already knows the request.
  def canRelease(r: str): bool =
    s.subscriptions.contains(r) or s.publishing == NOBODY

  def released(r: str): str -> Publication =
    if (not(s.subscriptions.contains(r))) s.outlets
    else if (s.outlets.get(r) == Published) s.outlets.set(r, Released)
    else s.outlets.keys().exclude(Set(r)).mapBy(k => s.outlets.get(k))

  action releaseSubscriber(r: str): bool = all {
    s.subscribers.get(r) == SUnsubscribing,
    canRelease(r),
    s' = {
      ...s,
      outlets: released(r),
      subscriptions: s.subscriptions.exclude(Set(r)),
      subscribers: s.subscribers.set(r, SUnsubscribed),
    },
  }

  action resubscribe(r: str): bool = all {
    Set(SUnsubscribed, SDone, SRejected).contains(s.subscribers.get(r)),
    s' = { ...s, subscribers: s.subscribers.set(r, SIdle) },
  }

  // `publish`, up to the PUBLISH_OK await: holds `publishing`; a no-op while
  // the peer already has an outlet.
  action publish(p: str): bool = all {
    s.registered,
    s.publishing == NOBODY,
    not(s.outlets.keys().contains(p)),
    s.peers.get(p) == PIdle,
    s' = { ...s, publishing: p, peers: s.peers.set(p, POffered) },
  }

  action peerAcceptsPublish(p: str): bool = all {
    s.peers.get(p) == POffered,
    s' = {
      ...s,
      registering: s.registering.union(Set(p)),
      peers: s.peers.set(p, PActive),
    },
  }

  action peerRefusesPublish(p: str): bool = all {
    s.peers.get(p) == POffered,
    s' = { ...s, publishing: NOBODY, peers: s.peers.set(p, PRefused) },
  }

  // `publish` after PUBLISH_OK: registers the outlet and takes the track up
  // again, then drops `publishing`.
  action registerPublish(p: str): bool = all {
    s.registering.contains(p),
    s' = {
      ...s,
      ended: false,
      outlets: s.outlets.put(p, Published),
      subscriptions: s.subscriptions.union(Set(p)),
      publishing: NOBODY,
      registering: s.registering.exclude(Set(p)),
    },
  }

  // `withdraw_publish` (and `end_track` first): the `publish` waiting for
  // PUBLISH_OK drops its request, fails, and drops `publishing`; the PUBLISH
  // answered or answered later gets PUBLISH_DONE.
  action withdrawPublish(p: str): bool = all {
    s.publishing == p,
    s' = {
      ...s,
      publishing: NOBODY,
      registering: s.registering.exclude(Set(p)),
      peers: s.peers.set(p, PDone),
    },
  }

  action peerUnsubscribes(p: str): bool = all {
    s.peers.get(p) == PActive,
    s' = { ...s, peers: s.peers.set(p, PUnsubscribing) },
  }

  action releasePeer(p: str): bool = all {
    s.peers.get(p) == PUnsubscribing,
    canRelease(p),
    s' = {
      ...s,
      outlets: released(p),
      subscriptions: s.subscriptions.exclude(Set(p)),
      peers: s.peers.set(p, PUnsubscribed),
    },
  }

  action republish(p: str): bool = all {
    Set(PUnsubscribed, PDone, PRefused).contains(s.peers.get(p)),
    s' = { ...s, peers: s.peers.set(p, PIdle) },
  }

  action step = any {
    ensureTrack,
    endTrack,
    nondet r = oneOf(SUBSCRIBERS)
    any {
      sendSubscribe(r),
      handleSubscribe(r),
      acceptSubscribe(r),
      sendUnsubscribe(r),
      releaseSubscriber(r),
      resubscribe(r),
    },
    nondet p = oneOf(PUBLISH_PEERS)
    any {
      publish(p),
      peerAcceptsPublish(p),
      peerRefusesPublish(p),
      registerPublish(p),
      withdrawPublish(p),
      peerUnsubscribes(p),
      releasePeer(p),
      republish(p),
    },
  }

  val endedTrackTakesNoSubscribe: bool =
    s.ended implies s.outlets.keys().forall(k => s.outlets.get(k) != Subscribed)

  // Every SUBSCRIBE is accepted or rejected with TRACK_DOES_NOT_EXIST, never
  // left to the handler's NOT_SUPPORTED.
  val originAnswersEverySubscribe: bool =
    SUBSCRIBERS.forall(r => s.subscribers.get(r) != SNotSupported)

  // A subscriber that received SUBSCRIBE_OK is served until it sends
  // UNSUBSCRIBE or receives PUBLISH_DONE.
  val activeSubscriberIsServed: bool =
    SUBSCRIBERS.forall(r =>
      s.subscribers.get(r) == SActive implies
        (s.outlets.keys().contains(r) and s.outlets.get(r) == Subscribed))

  // A peer that answered PUBLISH_OK is served once `publish` registered it.
  val activePeerIsServed: bool =
    PUBLISH_PEERS.forall(p =>
      (s.peers.get(p) == PActive and not(s.registering.contains(p))) implies
        (s.outlets.keys().contains(p) and s.outlets.get(p) == Published))

  // `subscriptions` names exactly the outlets still serving.
  val subscriptionsMatchOutlets: bool =
    s.subscriptions == s.outlets.keys().filter(k => s.outlets.get(k) != Released)

  val safety: bool = and {
    endedTrackTakesNoSubscribe,
    originAnswersEverySubscribe,
    activeSubscriberIsServed,
    activePeerIsServed,
    subscriptionsMatchOutlets,
  }
}

// expect: ok
module origin_current {
  import origin(
    SUBSCRIBERS = Set("s1", "s2"),
    PUBLISH_PEERS = Set("p1"),
    CHECK_ENDED = true,
    ATOMIC_HANDLE = true,
  ).*
}

// expect: violation
module origin_without_ended_check {
  import origin(
    SUBSCRIBERS = Set("s1", "s2"),
    PUBLISH_PEERS = Set("p1"),
    CHECK_ENDED = false,
    ATOMIC_HANDLE = true,
  ).*
}

// expect: violation
module origin_check_then_accept {
  import origin(
    SUBSCRIBERS = Set("s1", "s2"),
    PUBLISH_PEERS = Set("p1"),
    CHECK_ENDED = true,
    ATOMIC_HANDLE = false,
  ).*
}

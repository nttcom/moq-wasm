// A model of one track of `Origin` (crates/moqt/src/modules/moqt/domains/origin.rs)
// served to sessions that may both SUBSCRIBE to it and be sent a PUBLISH of
// it, with the Forward State of each outlet and the demand reported to a
// producer that publishes on demand. `origin.qnt` keeps subscribers and
// PUBLISH peers apart; here one session is both.
//
// Code mapping (origin.rs):
// - `OutletState` is the code's `OutletState`, whose `Serving` holds the
//   Forward State; `Demand` is `OriginTrack::demand`.
// - `publish` is two actions: `publish` checks that the session
//   has no outlet under `state`, then `state` is unlocked while
//   `publishing` stays held across the PUBLISH_OK await;
//   `registerPublish` pushes the Published outlet and calls `refresh_demand`,
//   or, with RECHECK_ON_REGISTER, sends PUBLISH_DONE when the session has
//   an outlet.
// - `accept_subscribe` is one action, `handleSubscribe`: it holds `state`
//   from the check to the outlet push, but never takes `publishing`.
//   SUBSCRIBE_ENDS_PUBLISH is `Outlet::end_publish`.
// - `apply_update` (:709-734) through `lock_state_knowing` (:817-825): an
//   update of a SUBSCRIBE is known at once; the model lets the peer update a
//   PUBLISH only once `publish` registered it, which is where the lock wait
//   puts it.
// - `release` (:831-845) through `Outlet::release` (:386-402): a Subscribed
//   outlet is removed, a Published one becomes Released; `refresh_demand`.
// - `end_track` (:855-882): waits for `publishing`, takes every outlet
//   (Released ones too), sets `demand` to None and `lifecycle` to Ended.
// - `publish_on_demand` (:490-509): reports `wanted()` (:299-304) and keeps it.
// - `refresh_demand` (:307-316): sends `wanted()` when it changed.
// Session disconnect is not modeled: the Origin is never told of a closed
// connection, but a client session's migration task turns every request of a
// connection it retires into an UNSUBSCRIBE the Origin releases
// (session/migration_task.rs `retire`, :320-335), and the final Disconnected
// ends the client session that owns the Origin (client/routes.rs :209).
//
// Switches:
// - PEER_SUBSCRIBES_WHILE_PUBLISHED: the peer may SUBSCRIBE to a track it
//   already accepted a PUBLISH of (the draft does not forbid it). Off in
//   `_current`, so its violation comes from crossing messages alone.
// - RECHECK_ON_REGISTER (half of the fix): after PUBLISH_OK, `publish`
//   checks again under `state` whether the session took the track by
//   SUBSCRIBE meanwhile, and then ends the new subscription with
//   PUBLISH_DONE instead of registering it.
// - SUBSCRIBE_ENDS_PUBLISH (other half): `accept_subscribe`, finding a
//   serving Published outlet of the same session, ends it with PUBLISH_DONE
//   and keeps it as Released. Needed because a SUBSCRIBE sent while the
//   PUBLISH was in flight can also be handled after `publish` registered.
// - REFRESH_ON_RELEASE: `release` calls `refresh_demand` (the code does).
//
// Commands (from docs/architecture/moqt/models, Java 21+ on PATH):
//   quint typecheck origin_sessions.qnt
//   quint run --main origin_sessions_current --invariant safety --max-samples 5000 --max-steps 30 origin_sessions.qnt
//   quint verify --backend tlc --main origin_sessions_current --invariant safety origin_sessions.qnt
//   (likewise for origin_sessions_recheck_on_register_only,
//   origin_sessions_fixed, origin_sessions_without_release_refresh)
module origin_sessions {
  const SESSIONS: Set[str]
  const PEER_SUBSCRIBES_WHILE_PUBLISHED: bool
  const RECHECK_ON_REGISTER: bool
  const SUBSCRIBE_ENDS_PUBLISH: bool
  const REFRESH_ON_RELEASE: bool

  // `Outlet` of one request: its `publication` and `forward`.
  type OutletState =
    | NoOutlet
    | Serving(bool)  // Subscribed or Published, with its Forward State
    | Released       // a PUBLISH the peer unsubscribed from

  // A SUBSCRIBE of the session, as the session sees it.
  type SubscribeState =
    | SIdle
    | SSent(bool)    // in flight, with its Forward State
    | SActive        // SUBSCRIBE_OK received
    | SUnsubscribing // UNSUBSCRIBE in flight

  // A PUBLISH sent to the session, as the session sees it.
  type PublishState =
    | PIdle
    | POffered        // PUBLISH in flight; `publishing` held
    | PAccepted(bool) // PUBLISH_OK sent with its Forward State; not registered yet
    | PActive
    | PUnsubscribing

  // `OriginTrack::demand`: None, or the last `TrackDemand.wanted` sent.
  type Demand = NoDemand | Reported(bool)

  type State = {
    ended: bool,
    subOutlets: str -> OutletState,
    pubOutlets: str -> OutletState,
    // The session whose `publish` holds `publishing`, or "".
    publishing: str,
    demand: Demand,
    subscribes: str -> SubscribeState,
    publishes: str -> PublishState,
  }

  var s: State

  pure val NOBODY = ""

  pure def isServing(o: OutletState): bool = match o {
    | Serving(_) => true
    | _ => false
  }

  pure def forwards(o: OutletState): bool = match o {
    | Serving(f) => f
    | _ => false
  }

  pure def hasOutlet(o: OutletState): bool = o != NoOutlet

  // `OriginTrack::wanted`
  def wantedIn(st: State): bool =
    SESSIONS.exists(x => forwards(st.subOutlets.get(x)) or forwards(st.pubOutlets.get(x)))

  // `OriginTrack::refresh_demand`
  def refreshed(st: State): State = match st.demand {
    | NoDemand => st
    | Reported(_) => { ...st, demand: Reported(wantedIn(st)) }
  }

  action init = s' = {
    ended: false,
    subOutlets: SESSIONS.mapBy(_ => NoOutlet),
    pubOutlets: SESSIONS.mapBy(_ => NoOutlet),
    publishing: NOBODY,
    demand: NoDemand,
    subscribes: SESSIONS.mapBy(_ => SIdle),
    publishes: SESSIONS.mapBy(_ => PIdle),
  }

  // `ensure_track` of the registered track.
  action ensureTrack = all {
    s.ended,
    s' = { ...s, ended: false },
  }

  action publishOnDemand = all {
    s.demand == NoDemand,
    s' = { ...s, demand: Reported(wantedIn(s)) },
  }

  action endTrack = all {
    s.publishing == NOBODY,
    s' = {
      ...s,
      ended: true,
      subOutlets: SESSIONS.mapBy(_ => NoOutlet),
      pubOutlets: SESSIONS.mapBy(_ => NoOutlet),
      demand: NoDemand,
      subscribes: SESSIONS.mapBy(x =>
        if (s.subscribes.get(x) == SActive) SIdle else s.subscribes.get(x)),
      publishes: SESSIONS.mapBy(x =>
        if (s.publishes.get(x) == PActive) PIdle else s.publishes.get(x)),
    },
  }

  // The session knows it takes the track by PUBLISH once it sent PUBLISH_OK.
  def peerTakesPublish(x: str): bool = match s.publishes.get(x) {
    | PIdle => false
    | POffered => false
    | _ => true
  }

  action sendSubscribe(x: str, forward: bool): bool = all {
    s.subscribes.get(x) == SIdle,
    PEER_SUBSCRIBES_WHILE_PUBLISHED or not(peerTakesPublish(x)),
    s' = { ...s, subscribes: s.subscribes.set(x, SSent(forward)) },
  }

  // `handle(SessionEvent::Subscribe)` → `accept_subscribe`, under `state`
  // only. An ended track is answered TRACK_DOES_NOT_EXIST by the caller.
  pure def sentForward(st: SubscribeState): bool = match st {
    | SSent(f) => f
    | _ => false
  }

  pure def isSent(st: SubscribeState): bool = match st {
    | SSent(_) => true
    | _ => false
  }

  action handleSubscribe(x: str): bool = all {
    isSent(s.subscribes.get(x)),
    if (s.ended) s' = { ...s, subscribes: s.subscribes.set(x, SIdle) }
    else if (SUBSCRIBE_ENDS_PUBLISH and isServing(s.pubOutlets.get(x))) s' = refreshed({
      ...s,
      subOutlets: s.subOutlets.set(x, Serving(sentForward(s.subscribes.get(x)))),
      // PUBLISH_DONE on the PUBLISH the session took.
      pubOutlets: s.pubOutlets.set(x, Released),
      subscribes: s.subscribes.set(x, SActive),
      publishes:
        if (s.publishes.get(x) == PActive) s.publishes.set(x, PIdle)
        else s.publishes,
    })
    else s' = refreshed({
      ...s,
      subOutlets: s.subOutlets.set(x, Serving(sentForward(s.subscribes.get(x)))),
      subscribes: s.subscribes.set(x, SActive),
    }),
  }

  // SUBSCRIBE_UPDATE / REQUEST_UPDATE through `apply_update`.
  action updateSubscribe(x: str, forward: bool): bool = all {
    s.subscribes.get(x) == SActive,
    isServing(s.subOutlets.get(x)),
    s' = refreshed({ ...s, subOutlets: s.subOutlets.set(x, Serving(forward)) }),
  }

  action sendUnsubscribe(x: str): bool = all {
    s.subscribes.get(x) == SActive,
    s' = { ...s, subscribes: s.subscribes.set(x, SUnsubscribing) },
  }

  def afterRelease(st: State): State = if (REFRESH_ON_RELEASE) refreshed(st) else st

  // `release` of the SUBSCRIBE. `lock_state_knowing` knows it at once.
  action releaseSubscribe(x: str): bool = all {
    s.subscribes.get(x) == SUnsubscribing,
    s' = afterRelease({
      ...s,
      subOutlets: s.subOutlets.set(x, NoOutlet),
      subscribes: s.subscribes.set(x, SIdle),
    }),
  }

  // `publish` up to the PUBLISH_OK await: a no-op while the session has an
  // outlet of the track (`Released` included).
  action publish(x: str): bool = all {
    s.publishing == NOBODY,
    s.publishes.get(x) == PIdle,
    not(hasOutlet(s.subOutlets.get(x))),
    not(hasOutlet(s.pubOutlets.get(x))),
    s' = { ...s, publishing: x, publishes: s.publishes.set(x, POffered) },
  }

  action peerAcceptsPublish(x: str, forward: bool): bool = all {
    s.publishes.get(x) == POffered,
    s' = { ...s, publishes: s.publishes.set(x, PAccepted(forward)) },
  }

  action peerRefusesPublish(x: str): bool = all {
    s.publishes.get(x) == POffered,
    s' = { ...s, publishing: NOBODY, publishes: s.publishes.set(x, PIdle) },
  }

  // `publish` after PUBLISH_OK: registers the Published outlet, takes the
  // track up again and drops `publishing`.
  pure def acceptedForward(st: PublishState): bool = match st {
    | PAccepted(f) => f
    | _ => false
  }

  pure def isAccepted(st: PublishState): bool = match st {
    | PAccepted(_) => true
    | _ => false
  }

  action registerPublish(x: str): bool = all {
    isAccepted(s.publishes.get(x)),
    if (RECHECK_ON_REGISTER and hasOutlet(s.subOutlets.get(x)))
      // PUBLISH_DONE on the new subscription; nothing registered.
      s' = { ...s, publishing: NOBODY, publishes: s.publishes.set(x, PIdle) }
    else s' = refreshed({
      ...s,
      ended: false,
      pubOutlets: s.pubOutlets.set(x, Serving(acceptedForward(s.publishes.get(x)))),
      publishing: NOBODY,
      publishes: s.publishes.set(x, PActive),
    }),
  }

  action updatePublish(x: str, forward: bool): bool = all {
    s.publishes.get(x) == PActive,
    isServing(s.pubOutlets.get(x)),
    s' = refreshed({ ...s, pubOutlets: s.pubOutlets.set(x, Serving(forward)) }),
  }

  action peerUnsubscribesPublish(x: str): bool = all {
    s.publishes.get(x) == PActive,
    s' = { ...s, publishes: s.publishes.set(x, PUnsubscribing) },
  }

  // `release` of the PUBLISH: its outlet stays as Released.
  action releasePublish(x: str): bool = all {
    s.publishes.get(x) == PUnsubscribing,
    s' = afterRelease({
      ...s,
      pubOutlets:
        if (isServing(s.pubOutlets.get(x))) s.pubOutlets.set(x, Released)
        else s.pubOutlets,
      publishes: s.publishes.set(x, PIdle),
    }),
  }

  action step = any {
    ensureTrack,
    publishOnDemand,
    endTrack,
    nondet x = oneOf(SESSIONS)
    nondet forward = oneOf(Set(true, false))
    any {
      sendSubscribe(x, forward),
      handleSubscribe(x),
      updateSubscribe(x, forward),
      sendUnsubscribe(x),
      releaseSubscribe(x),
      publish(x),
      peerAcceptsPublish(x, forward),
      peerRefusesPublish(x),
      registerPublish(x),
      updatePublish(x, forward),
      peerUnsubscribesPublish(x),
      releasePublish(x),
    },
  }

  // A session is served the track by at most one outlet, so it is sent each
  // object at most once. `publish` is documented as a no-op while the peer
  // takes the track.
  val oneOutletPerSession: bool =
    SESSIONS.forall(x =>
      not(isServing(s.subOutlets.get(x)) and isServing(s.pubOutlets.get(x))))

  // While the track is published on demand, the producer was last told
  // whether some serving outlet is in Forward State 1.
  val demandMatchesOutlets: bool = match s.demand {
    | NoDemand => true
    | Reported(w) => w == wantedIn(s)
  }

  val safety: bool = and {
    oneOutletPerSession,
    demandMatchesOutlets,
  }
}

// expect: ok
module origin_sessions_current {
  import origin_sessions(
    SESSIONS = Set("a", "b"),
    PEER_SUBSCRIBES_WHILE_PUBLISHED = false,
    RECHECK_ON_REGISTER = true,
    SUBSCRIBE_ENDS_PUBLISH = true,
    REFRESH_ON_RELEASE = true,
  ).*
}

// Checking again after PUBLISH_OK alone misses a crossing SUBSCRIBE handled
// after `publish` registered.
// expect: violation
module origin_sessions_recheck_on_register_only {
  import origin_sessions(
    SESSIONS = Set("a", "b"),
    PEER_SUBSCRIBES_WHILE_PUBLISHED = false,
    RECHECK_ON_REGISTER = true,
    SUBSCRIBE_ENDS_PUBLISH = false,
    REFRESH_ON_RELEASE = true,
  ).*
}

// Also covers a peer that SUBSCRIBEs to a track it already takes by PUBLISH.
// expect: ok
module origin_sessions_fixed {
  import origin_sessions(
    SESSIONS = Set("a", "b"),
    PEER_SUBSCRIBES_WHILE_PUBLISHED = true,
    RECHECK_ON_REGISTER = true,
    SUBSCRIBE_ENDS_PUBLISH = true,
    REFRESH_ON_RELEASE = true,
  ).*
}

// expect: violation
module origin_sessions_without_release_refresh {
  import origin_sessions(
    SESSIONS = Set("a", "b"),
    PEER_SUBSCRIBES_WHILE_PUBLISHED = true,
    RECHECK_ON_REGISTER = true,
    SUBSCRIBE_ENDS_PUBLISH = true,
    REFRESH_ON_RELEASE = false,
  ).*
}

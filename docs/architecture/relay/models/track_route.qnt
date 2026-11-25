// A model of the route a relay owns in the route registry for one track its
// clients publish with PUBLISH alone (`route:publisher:track:<ns>:<name>`):
// control_plane/sequences/publish.rs (`register_route`),
// cascading/namespace_routes.rs (`withdraw_track`) and its callers
// upstream_publish_done.rs, session_workers/session_cleanup.rs,
// malformed_track.rs, drain_release.rs and sequences.rs
// (`release_upstream_subscriptions`), with the registry in
// cascading/route_registry/redis.rs.
//
// Client sessions PUBLISH the track and end it. Each session's worker
// handles its events one at a time, and the workers run concurrently. Each
// action is one step between two awaits: a change of the directory (the
// track's DashMap entry), the directory check `has_client_published_track`,
// or one route registry call. The registry holds `owned_routes` across its
// Redis call, so register (an upsert) and unregister are each one step; the
// heartbeat restores only the routes still in `owned_routes`, which both
// calls change together with Redis, so the route is one boolean. A failed
// unregister has already dropped the route from `owned_routes`, so it
// expires: it is false here. A withdrawal's check and its unregister are two
// steps: the unregister queues for `owned_routes`, a fair tokio mutex, in
// the poll that ran the check, so a registry call queued later waits for it
// unless the coop budget makes that poll yield first; the two steps cover
// that yield. Relay peers, which own no route, other relays,
// the cache and the draining flag are left out; the releases that happen
// only while draining are allowed at any time.
//
// Simulate: npx @informalsystems/quint run --main relay_track_route --invariant safety track_route.qnt
// Check every run up to N steps (Apalache, Java 21+):
//   npx @informalsystems/quint verify --main relay_track_route --invariant safety --max-steps N track_route.qnt
module track_route {
  const CLIENTS: Set[str]

  // A client's PUBLISH of the track, or a withdrawal its worker runs.
  type Phase =
    | Idle
    | Registered     // `register_route` succeeded; joins the directory next
    | Joined         // `register_published_subscription` done; PUBLISH_OK next
    | Member         // answered PUBLISH_OK
    | Checking       // a withdrawal: `has_client_published_track` next
    | Unregistering  // passed the check: `unregister_track_publisher` next

  type State = {
    // The clients whose PUBLISH producer is in the directory.
    members: Set[str],
    // Whether `owned_routes` holds the track's route.
    route: bool,
    phase: str -> Phase,
    // The worker of a subscriber session whose UNSUBSCRIBE or cleanup
    // released every PUBLISH while the relay drains
    // (`end_downstream_subscription`, then `release_upstream_subscriptions`).
    releaser: Phase,
  }

  var s: State

  action init = s' = {
    members: Set(),
    route: false,
    phase: CLIENTS.mapBy(_ => Idle),
    releaser: Idle,
  }

  // `has_client_published_track`: a withdrawal stops while a client
  // publishes the track and unregisters otherwise.
  pure def check(members: Set[str]): Phase =
    if (members != Set()) Idle else Unregistering

  def setPhase(c: str, p: Phase): str -> Phase = s.phase.set(c, p)

  // --- PUBLISH -----------------------------------------------------------

  // `register_route`, an upsert; a failure answers PUBLISH_ERROR.
  action publish(c: str): bool = all {
    s.phase.get(c) == Idle,
    nondet registered = oneOf(Set(true, false))
    s' = {
      ...s,
      route: s.route or registered,
      phase: setPhase(c, if (registered) Registered else Idle),
    },
  }

  // `register_upstream_subscription`. It fails, answering PUBLISH_ERROR and
  // withdrawing the route, when `start_ingest` finds no publisher session;
  // only the session's own cleanup removes it, so a client never takes this
  // branch today.
  action publishJoin(c: str): bool = all {
    s.phase.get(c) == Registered,
    nondet ingested = oneOf(Set(true, false))
    s' =
      if (ingested) { ...s, members: s.members.union(Set(c)), phase: setPhase(c, Joined) }
      else { ...s, phase: setPhase(c, Checking) },
  }

  // `notify_namespace_subscribers`, then PUBLISH_OK.
  action publishOk(c: str): bool = all {
    s.phase.get(c) == Joined,
    s' = { ...s, phase: setPhase(c, Member) },
  }

  // --- Ends of a PUBLISH ---------------------------------------------------

  // PUBLISH_DONE (`UpstreamPublishDone`): `end_upstream_subscription` and the
  // check run without an await between them. A PUBLISH the relay already
  // ended finds no producer and withdraws nothing.
  action publishDone(c: str): bool = all {
    s.phase.get(c) == Member,
    if (s.members.contains(c))
      val left = s.members.exclude(Set(c))
      s' = { ...s, members: left, phase: setPhase(c, check(left)) }
    else s' = { ...s, phase: setPhase(c, Idle) },
  }

  // PUBLISH_DONE of a subscription the client serves for the relay's
  // SUBSCRIBE: it withdraws the route too, as `withdraw_track` does
  // not ask how the ended subscription began.
  action subscribedPublishDone(c: str): bool = all {
    s.phase.get(c) == Idle,
    s' = { ...s, phase: setPhase(c, check(s.members)) },
  }

  // `cleanup_session`: `remove_session` drops the producer; the check comes
  // after the awaits of the session's downstream subscriptions. The session
  // that replaces it starts idle.
  action disconnect(c: str): bool = all {
    s.phase.get(c) == Member,
    s.members.contains(c),
    s' = { ...s, members: s.members.exclude(Set(c)), phase: setPhase(c, Checking) },
  }

  // `ReleaseUnwatchedPublishes` after GOAWAY: drops the client's producer,
  // sends UNSUBSCRIBE, then withdraws.
  action drainRelease(c: str): bool = all {
    s.phase.get(c) == Member,
    s.members.contains(c),
    s' = { ...s, members: s.members.exclude(Set(c)), phase: setPhase(c, Checking) },
  }

  // `MalformedTrackCleanup` in the worker of a publisher of the track:
  // `remove_upstream_track` drops every producer, the UNSUBSCRIBEs follow,
  // then the withdrawal. The other clients' PUBLISHes end too.
  action malformed(c: str): bool = all {
    s.phase.get(c) == Member,
    s.members.contains(c),
    s' = { ...s, members: Set(), phase: setPhase(c, Checking) },
  }

  action checkRoute(c: str): bool = all {
    s.phase.get(c) == Checking,
    s' = { ...s, phase: setPhase(c, check(s.members)) },
  }

  action unregister(c: str): bool = all {
    s.phase.get(c) == Unregistering,
    s' = { ...s, route: false, phase: setPhase(c, Idle) },
  }

  // --- A subscriber's release while draining -------------------------------

  action releaseAll: bool = all {
    s.releaser == Idle,
    s.members != Set(),
    s' = { ...s, members: Set(), releaser: Checking },
  }

  action releaserCheck: bool = all {
    s.releaser == Checking,
    s' = { ...s, releaser: check(s.members) },
  }

  action releaserUnregister: bool = all {
    s.releaser == Unregistering,
    s' = { ...s, route: false, releaser: Idle },
  }

  action step = any {
    nondet c = oneOf(CLIENTS)
    any {
      publish(c),
      publishJoin(c),
      publishOk(c),
      publishDone(c),
      subscribedPublishDone(c),
      disconnect(c),
      drainRelease(c),
      malformed(c),
      checkRoute(c),
      unregister(c),
    },
    releaseAll,
    releaserCheck,
    releaserUnregister,
  }

  // --- Properties --------------------------------------------------------

  // Every worker waits for its next event.
  val quiescent: bool = and {
    CLIENTS.forall(c => Set(Idle, Member).contains(s.phase.get(c))),
    s.releaser == Idle,
  }

  // A client answered PUBLISH_OK whose PUBLISH has not ended has the route,
  // so clients of other relays can subscribe to the track.
  val publishedTrackHasRoute: bool =
    CLIENTS.forall(c => (s.phase.get(c) == Member and s.members.contains(c)) implies s.route)

  // With nothing in flight, the route is held exactly while a client
  // publishes the track.
  val quiescentRouteMatchesDirectory: bool =
    quiescent implies s.route == (s.members != Set())

  val safety: bool = and {
    publishedTrackHasRoute,
    quiescentRouteMatchesDirectory,
  }
}

module relay_track_route {
  import track_route(CLIENTS = Set("a", "b", "c")).*
}

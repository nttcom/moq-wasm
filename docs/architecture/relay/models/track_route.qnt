// A model of the route a relay owns in the route registry for one track its
// clients publish with PUBLISH alone (`route:publisher:track:<ns>:<name>`):
// cascading/namespace_routes.rs (`reconcile_track`, `withdraw_track`),
// control_plane/sequences/publish.rs (`register_route`) and the callers that
// reconcile after a PUBLISH left the directory: upstream_publish_done.rs,
// session_workers/session_cleanup.rs, malformed_track.rs, drain_release.rs
// and sequences.rs (`release_upstream_subscriptions`).
//
// Client sessions PUBLISH the track and end it. Each session's worker
// handles its events one at a time, and the workers run concurrently. Each
// action is one step between two awaits: a change of the directory (the
// track's DashMap entry) or one reconcile, which reads the directory and
// calls the route registry under the track route's lock, so it is one step.
// `route` is whether `NamespaceRoutes` holds the route; the registry's
// heartbeat restores exactly the routes it holds. Relay peers, which own no
// route, other relays, the cache and the draining flag are left out; the
// releases that happen only while draining are allowed at any time.
//
// Simulate: npx @informalsystems/quint run --main relay_track_route --invariant safety track_route.qnt
// Check every run up to N steps (Apalache, Java 21+):
//   npx @informalsystems/quint verify --main relay_track_route --invariant safety --max-steps N track_route.qnt
module track_route {
  const CLIENTS: Set[str]

  // A client's PUBLISH of the track, or a withdrawal its worker runs.
  type Phase =
    | Idle
    | Joining      // `register_published_subscription` done; `reconcile_track` next
    | Joined       // the reconcile registered the route; PUBLISH_OK next
    | Member       // answered PUBLISH_OK
    | RollingBack  // its reconcile failed; `end_upstream_subscription` next
    | Reverting    // left the directory after the failure; `withdraw_track` next
    | Leaving      // a withdrawal: `withdraw_track` next

  type State = {
    // The clients whose PUBLISH producer is in the directory.
    members: Set[str],
    // Whether the relay holds the track's route.
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

  // `NamespaceRoutes::reconcile`: registers the route while a client
  // publishes the track and it is not held, withdraws it once none does.
  // Returns the route and whether the reconcile succeeded.
  pure def reconcile(members: Set[str], route: bool, registered: bool): { route: bool, ok: bool } =
    val wanted = members != Set()
    if (wanted and not(route)) { route: registered, ok: registered }
    else { route: wanted, ok: true }

  def setPhase(c: str, p: Phase): str -> Phase = s.phase.set(c, p)

  // --- PUBLISH -----------------------------------------------------------

  // `register_upstream_subscription`. It fails, answering PUBLISH_ERROR, when
  // `start_ingest` finds no publisher session; only the session's own
  // cleanup removes it, so a client never takes this branch today.
  action publish(c: str): bool = all {
    s.phase.get(c) == Idle,
    nondet ingested = oneOf(Set(true, false))
    s' =
      if (ingested) { ...s, members: s.members.union(Set(c)), phase: setPhase(c, Joining) }
      else s,
  }

  // Every reconcile of a client's worker: its PUBLISH's (`register_route`),
  // the one after a failed one, and a withdrawal's. A registration may fail.
  action reconcileClient(c: str): bool = all {
    Set(Joining, Reverting, Leaving).contains(s.phase.get(c)),
    nondet registered = oneOf(Set(true, false))
    val r = reconcile(s.members, s.route, registered)
    val after =
      if (s.phase.get(c) != Joining) Idle
      else if (r.ok) Joined
      else RollingBack
    s' = { ...s, route: r.route, phase: setPhase(c, after) },
  }

  // `end_upstream_subscription` of the PUBLISH whose route failed; the
  // PUBLISH_ERROR follows the next reconcile.
  action rollBack(c: str): bool = all {
    s.phase.get(c) == RollingBack,
    s' = { ...s, members: s.members.exclude(Set(c)), phase: setPhase(c, Reverting) },
  }

  // `notify_namespace_subscribers`, then PUBLISH_OK.
  action publishOk(c: str): bool = all {
    s.phase.get(c) == Joined,
    s' = { ...s, phase: setPhase(c, Member) },
  }

  // --- Ends of a PUBLISH ---------------------------------------------------

  // PUBLISH_DONE (`UpstreamPublishDone`): `end_upstream_subscription`, then
  // the reconcile.
  action publishDone(c: str): bool = all {
    s.phase.get(c) == Member,
    s' = { ...s, members: s.members.exclude(Set(c)), phase: setPhase(c, Leaving) },
  }

  // PUBLISH_DONE of a subscription the client serves for the relay's
  // SUBSCRIBE reconciles the route too.
  action subscribedPublishDone(c: str): bool = all {
    s.phase.get(c) == Idle,
    s' = { ...s, phase: setPhase(c, Leaving) },
  }

  // `cleanup_session`: `remove_session` drops the producer; the reconcile
  // comes after the awaits of the session's downstream subscriptions. The
  // session that replaces it starts idle.
  action disconnect(c: str): bool = all {
    s.phase.get(c) == Member,
    s.members.contains(c),
    s' = { ...s, members: s.members.exclude(Set(c)), phase: setPhase(c, Leaving) },
  }

  // `ReleaseUnwatchedPublishes` after GOAWAY: drops the client's producer,
  // sends UNSUBSCRIBE, then reconciles.
  action drainRelease(c: str): bool = all {
    s.phase.get(c) == Member,
    s.members.contains(c),
    s' = { ...s, members: s.members.exclude(Set(c)), phase: setPhase(c, Leaving) },
  }

  // `MalformedTrackCleanup` in the worker of a publisher of the track:
  // `remove_upstream_track` drops every producer, the UNSUBSCRIBEs follow,
  // then the reconcile. The other clients' PUBLISHes end too.
  action malformed(c: str): bool = all {
    s.phase.get(c) == Member,
    s.members.contains(c),
    s' = { ...s, members: Set(), phase: setPhase(c, Leaving) },
  }

  // --- A subscriber's release while draining -------------------------------

  action releaseAll: bool = all {
    s.releaser == Idle,
    s.members != Set(),
    s' = { ...s, members: Set(), releaser: Leaving },
  }

  action releaserReconcile: bool = all {
    s.releaser == Leaving,
    nondet registered = oneOf(Set(true, false))
    s' = { ...s, route: reconcile(s.members, s.route, registered).route, releaser: Idle },
  }

  action step = any {
    nondet c = oneOf(CLIENTS)
    any {
      publish(c),
      reconcileClient(c),
      rollBack(c),
      publishOk(c),
      publishDone(c),
      subscribedPublishDone(c),
      disconnect(c),
      drainRelease(c),
      malformed(c),
    },
    releaseAll,
    releaserReconcile,
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

// A model of the namespace routes one relay owns in the route registry
// (services/relay/src/modules/cascading/namespace_routes.rs and its callers
// control_plane/sequences/publish_namespace.rs, publish_namespace_done.rs,
// subscribe_namespace.rs, unsubscribe_namespace.rs and
// session_workers/session_cleanup.rs).
//
// Client sessions publish one namespace and subscribe to one prefix. Each
// session handles its requests one at a time, and the sessions run
// concurrently. Each action is one step between two awaits: a change of the
// directory (`InMemoryLocalPubSubDirectory`, a DashMap shard lock) or one
// reconcile. A reconcile (`NamespaceRoutes::reconcile_publisher` /
// `reconcile_subscriber`) reads the directory under the route's lock and
// registers or withdraws the route to match; the lock is held across the
// route registry call, so one reconcile is one step. The heartbeat restores
// only the routes still in `owned_routes`, which register and unregister
// change together with Redis, so a route is one boolean here. Relay
// sessions, which own no route, and other relays are left out.
//
// Simulate: npx @informalsystems/quint run --main relay_namespace --invariant safety namespace.qnt
// Check every run up to 8 steps (Apalache, Java 21+):
//   npx @informalsystems/quint verify --main relay_namespace --invariant safety --max-steps 8 namespace.qnt
module namespace {
  const CLIENTS: Set[str]

  // The progress of a client's PUBLISH_NAMESPACE of the namespace or
  // SUBSCRIBE_NAMESPACE of the prefix.
  type Phase =
    | Idle
    | Joining      // in the directory, reconcile next
    | Member       // answered OK
    | RollingBack  // its reconcile failed; it leaves the directory next
    | Reverting    // left the directory after the failure, reconcile next
    | Leaving      // left the directory as its last client, reconcile next

  // The namespace's client publishers or the prefix's client subscribers in
  // the directory, and the route they own.
  type Side = { members: Set[str], route: bool, phase: str -> Phase }

  // `cleanup_session` after the directory dropped the session: the
  // subscription prefix is reconciled first, then the namespace.
  type Cleanup = CNone | CPrefix | CNamespace

  type State = {
    pub: Side,
    sub: Side,
    cleanup: str -> Cleanup,
    // The sessions whose cleanup reconciles the namespace or the prefix:
    // those that were its last client.
    cleanupReconcilesNamespace: Set[str],
    cleanupReconcilesPrefix: Set[str],
  }

  var s: State

  pure def emptySide(clients: Set[str]): Side =
    { members: Set(), route: false, phase: clients.mapBy(_ => Idle) }

  action init = s' = {
    pub: emptySide(CLIENTS),
    sub: emptySide(CLIENTS),
    cleanup: CLIENTS.mapBy(_ => CNone),
    cleanupReconcilesNamespace: Set(),
    cleanupReconcilesPrefix: Set(),
  }

  pure val inFlight = Set(Joining, RollingBack, Reverting, Leaving)

  def busy(c: str): bool =
    s.cleanup.get(c) != CNone or
    inFlight.contains(s.pub.phase.get(c)) or
    inFlight.contains(s.sub.phase.get(c))

  // `registered` is whether the registry call succeeds when the reconcile
  // registers.
  pure def reconcile(side: Side, registered: bool): { side: Side, ok: bool } =
    val wanted = side.members != Set()
    if (wanted and not(side.route))
      { side: { ...side, route: registered }, ok: registered }
    else
      { side: { ...side, route: wanted }, ok: true }

  // `register_publish_namespace` / `register_subscribe_namespace`.
  pure def join(side: Side, c: str): Side =
    { ...side, members: side.members.union(Set(c)), phase: side.phase.set(c, Joining) }

  // `unregister_*`; only a last client's departure reconciles the route.
  pure def leave(side: Side, c: str): Side =
    val remaining = side.members.exclude(Set(c))
    { ...side, members: remaining, phase: side.phase.set(c, if (remaining == Set()) Leaving else Idle) }

  // A reconcile after the client's own directory change: a joining client is
  // answered OK when the reconcile succeeds and rolls back when it fails.
  pure def settle(side: Side, c: str, registered: bool): Side =
    val r = reconcile(side, registered)
    val phase =
      if (side.phase.get(c) != Joining) Idle
      else if (r.ok) Member
      else RollingBack
    { ...r.side, phase: r.side.phase.set(c, phase) }

  pure def rollBack(side: Side, c: str): Side =
    { ...side, members: side.members.exclude(Set(c)), phase: side.phase.set(c, Reverting) }

  pure def reconciling(side: Side, c: str): bool =
    Set(Joining, Reverting, Leaving).contains(side.phase.get(c))

  // --- PUBLISH_NAMESPACE / PUBLISH_NAMESPACE_DONE --------------------------

  action publishNamespace(c: str): bool = all {
    not(busy(c)), s.pub.phase.get(c) == Idle,
    s' = { ...s, pub: join(s.pub, c) },
  }

  action publishNamespaceDone(c: str): bool = all {
    not(busy(c)), s.pub.phase.get(c) == Member,
    s' = { ...s, pub: leave(s.pub, c) },
  }

  action reconcilePublisher(c: str): bool = all {
    reconciling(s.pub, c),
    nondet registered = oneOf(Set(true, false))
    s' = { ...s, pub: settle(s.pub, c, registered) },
  }

  action rollBackPublisher(c: str): bool = all {
    s.pub.phase.get(c) == RollingBack,
    s' = { ...s, pub: rollBack(s.pub, c) },
  }

  // --- SUBSCRIBE_NAMESPACE / UNSUBSCRIBE_NAMESPACE -------------------------

  action subscribeNamespace(c: str): bool = all {
    not(busy(c)), s.sub.phase.get(c) == Idle,
    s' = { ...s, sub: join(s.sub, c) },
  }

  action unsubscribeNamespace(c: str): bool = all {
    not(busy(c)), s.sub.phase.get(c) == Member,
    s' = { ...s, sub: leave(s.sub, c) },
  }

  action reconcileSubscriber(c: str): bool = all {
    reconciling(s.sub, c),
    nondet registered = oneOf(Set(true, false))
    s' = { ...s, sub: settle(s.sub, c, registered) },
  }

  action rollBackSubscriber(c: str): bool = all {
    s.sub.phase.get(c) == RollingBack,
    s' = { ...s, sub: rollBack(s.sub, c) },
  }

  // --- Session cleanup ---------------------------------------------------

  // `remove_session`, between requests of the session.
  action disconnect(c: str): bool = all {
    not(busy(c)),
    val publishers = s.pub.members.exclude(Set(c))
    val subscribers = s.sub.members.exclude(Set(c))
    s' = {
      pub: { ...s.pub, members: publishers, phase: s.pub.phase.set(c, Idle) },
      sub: { ...s.sub, members: subscribers, phase: s.sub.phase.set(c, Idle) },
      cleanup: s.cleanup.set(c, CPrefix),
      cleanupReconcilesNamespace:
        if (s.pub.members.contains(c) and publishers == Set())
          s.cleanupReconcilesNamespace.union(Set(c))
        else s.cleanupReconcilesNamespace,
      cleanupReconcilesPrefix:
        if (s.sub.members.contains(c) and subscribers == Set())
          s.cleanupReconcilesPrefix.union(Set(c))
        else s.cleanupReconcilesPrefix,
    },
  }

  action cleanupPrefix(c: str): bool = all {
    s.cleanup.get(c) == CPrefix,
    nondet registered = oneOf(Set(true, false))
    s' = {
      ...s,
      sub: if (s.cleanupReconcilesPrefix.contains(c)) reconcile(s.sub, registered).side else s.sub,
      cleanupReconcilesPrefix: s.cleanupReconcilesPrefix.exclude(Set(c)),
      cleanup: s.cleanup.set(c, CNamespace),
    },
  }

  action cleanupNamespace(c: str): bool = all {
    s.cleanup.get(c) == CNamespace,
    nondet registered = oneOf(Set(true, false))
    s' = {
      ...s,
      pub: if (s.cleanupReconcilesNamespace.contains(c)) reconcile(s.pub, registered).side else s.pub,
      cleanupReconcilesNamespace: s.cleanupReconcilesNamespace.exclude(Set(c)),
      cleanup: s.cleanup.set(c, CNone),
    },
  }

  action step = {
    nondet c = oneOf(CLIENTS)
    any {
      publishNamespace(c),
      publishNamespaceDone(c),
      reconcilePublisher(c),
      rollBackPublisher(c),
      subscribeNamespace(c),
      unsubscribeNamespace(c),
      reconcileSubscriber(c),
      rollBackSubscriber(c),
      disconnect(c),
      cleanupPrefix(c),
      cleanupNamespace(c),
    }
  }

  // --- Properties --------------------------------------------------------

  pure def membersHaveRoute(side: Side, clients: Set[str]): bool =
    clients.exists(c => side.phase.get(c) == Member) implies side.route

  // A namespace a client was answered PUBLISH_NAMESPACE_OK for has its route,
  // so clients of other relays can subscribe to it.
  val publishedNamespaceHasRoute: bool = membersHaveRoute(s.pub, CLIENTS)

  // A prefix a client was answered SUBSCRIBE_NAMESPACE_OK for has its route,
  // so publishers on other relays announce to it.
  val subscribedPrefixHasRoute: bool = membersHaveRoute(s.sub, CLIENTS)

  // With every session idle, each route is held exactly while a client
  // needs it.
  val idleRelayRoutesMatchDirectory: bool =
    CLIENTS.forall(c => not(busy(c))) implies and {
      s.pub.route == (s.pub.members != Set()),
      s.sub.route == (s.sub.members != Set()),
    }

  val safety: bool = and {
    publishedNamespaceHasRoute,
    subscribedPrefixHasRoute,
    idleRelayRoutesMatchDirectory,
  }
}

module relay_namespace {
  import namespace(CLIENTS = Set("a", "b", "c")).*
}

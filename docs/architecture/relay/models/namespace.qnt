// A model of the namespace routes one relay owns in the route registry
// (services/relay/src/modules/control_plane/sequences/publish_namespace.rs,
// publish_namespace_done.rs, subscribe_namespace.rs, unsubscribe_namespace.rs
// and session_workers/session_cleanup.rs).
//
// Client sessions publish one namespace and subscribe to one prefix. Each
// session handles its requests one at a time, and the sessions run
// concurrently. Each action is one step between two awaits: a change of the
// directory (`InMemoryLocalPubSubDirectory`, a DashMap shard lock) or one
// route registry call (Redis, serialized by `owned_routes`). The heartbeat
// restores only the routes still in `owned_routes`, which register and
// unregister change together with Redis, so a route is one boolean here.
// Relay sessions, which own no route, and other relays are left out.
//
// The publisher route is `NamespaceRoutes::reconcile_publisher`
// (cascading/namespace_routes.rs): under the namespace's lock it reads the
// directory and registers or withdraws the route to match. The lock is held
// across the registry call, so one reconcile is one step.
//
// Simulate: npx @informalsystems/quint run --main relay_namespace --invariant safety namespace.qnt
// Check every run up to 8 steps (Apalache, Java 21+):
//   npx @informalsystems/quint verify --main relay_namespace --invariant safety --max-steps 8 namespace.qnt
module namespace {
  const CLIENTS: Set[str]

  // A client's PUBLISH_NAMESPACE of the namespace.
  type PublishState =
    | PIdle
    | PJoining       // in the directory, reconcile next
    | PPublished     // PUBLISH_NAMESPACE_OK sent
    | PRollingBack   // its reconcile failed; it leaves the directory next
    | PReverting     // left the directory after the failure, reconcile next
    | PLeaving       // left the directory as its last client, reconcile next

  // A client's SUBSCRIBE_NAMESPACE of the prefix.
  type SubscribeState =
    | SIdle
    | SRouting       // in the directory as its first client, `register_namespace_subscriber` in flight
    | SSubscribed    // SUBSCRIBE_NAMESPACE_OK sent
    | SRejected      // answered NAMESPACE_PREFIX_OVERLAP
    | SFailed        // the route could not be registered
    | SWithdrawing   // left the directory as its last client, `unregister_namespace_subscriber` next

  // `cleanup_session` after the directory dropped the session: the
  // subscription prefixes are withdrawn first, then the namespaces.
  type Cleanup = CNone | CPrefix | CNamespace

  type State = {
    // `publisher_namespaces` of the namespace
    publishers: Set[str],
    // `subscriber_namespaces` of the prefix
    subscribers: Set[str],
    publisherRoute: bool,
    subscriberRoute: bool,
    publish: str -> PublishState,
    subscribe: str -> SubscribeState,
    cleanup: str -> Cleanup,
    // The routes a session's cleanup reconciles or withdraws: its namespace
    // and its prefix when it was their last client.
    cleanupWithdrawsNamespace: Set[str],
    cleanupWithdrawsPrefix: Set[str],
  }

  var s: State

  action init = s' = {
    publishers: Set(),
    subscribers: Set(),
    publisherRoute: false,
    subscriberRoute: false,
    publish: CLIENTS.mapBy(_ => PIdle),
    subscribe: CLIENTS.mapBy(_ => SIdle),
    cleanup: CLIENTS.mapBy(_ => CNone),
    cleanupWithdrawsNamespace: Set(),
    cleanupWithdrawsPrefix: Set(),
  }

  def busy(c: str): bool =
    s.cleanup.get(c) != CNone or
    Set(PJoining, PRollingBack, PReverting, PLeaving).contains(s.publish.get(c)) or
    Set(SRouting, SWithdrawing).contains(s.subscribe.get(c))

  // --- PUBLISH_NAMESPACE -------------------------------------------------

  // `reconcile_publisher`; `registered` is whether the registry call, if
  // one is made, succeeds. Returns the route and whether it succeeded.
  pure def reconcilePublisher(
    publishers: Set[str], route: bool, registered: bool
  ): { route: bool, ok: bool } =
    if (publishers != Set() and not(route)) { route: registered, ok: registered }
    else { route: publishers != Set(), ok: true }

  // `register_publish_namespace`.
  action receivePublishNamespace(c: str): bool = all {
    not(busy(c)),
    s.publish.get(c) == PIdle,
    s' = {
      ...s,
      publishers: s.publishers.union(Set(c)),
      publish: s.publish.set(c, PJoining),
    },
  }

  // PUBLISH_NAMESPACE_OK once the route is registered; a Redis failure
  // withdraws the publication and answers PUBLISH_NAMESPACE_ERROR. Every
  // directory change is followed by a reconcile.
  action reconcilePublisherRoute(c: str): bool = all {
    Set(PJoining, PReverting, PLeaving).contains(s.publish.get(c)),
    nondet registered = oneOf(Set(true, false))
    val r = reconcilePublisher(s.publishers, s.publisherRoute, registered)
    val phase =
      if (s.publish.get(c) != PJoining) PIdle
      else if (r.ok) PPublished
      else PRollingBack
    s' = { ...s, publisherRoute: r.route, publish: s.publish.set(c, phase) },
  }

  action rollBackPublisher(c: str): bool = all {
    s.publish.get(c) == PRollingBack,
    s' = {
      ...s,
      publishers: s.publishers.exclude(Set(c)),
      publish: s.publish.set(c, PReverting),
    },
  }

  // `unregister_publish_namespace`; `NoClientsRemain` reconciles the route.
  action receivePublishNamespaceDone(c: str): bool = all {
    not(busy(c)),
    s.publish.get(c) == PPublished,
    val remaining = s.publishers.exclude(Set(c))
    s' = {
      ...s,
      publishers: remaining,
      publish: s.publish.set(c, if (remaining == Set()) PLeaving else PIdle),
    },
  }

  // --- SUBSCRIBE_NAMESPACE -----------------------------------------------

  // `register_subscribe_namespace`; only its first client registers the
  // route, every other one is answered SUBSCRIBE_NAMESPACE_OK at once.
  action receiveSubscribeNamespace(c: str): bool = all {
    not(busy(c)),
    Set(SIdle, SRejected, SFailed).contains(s.subscribe.get(c)),
    val first = s.subscribers == Set()
    s' = {
      ...s,
      subscribers: s.subscribers.union(Set(c)),
      subscribe: s.subscribe.set(c, if (first) SRouting else SSubscribed),
    },
  }

  // `register_route`: the script refuses a prefix the relay already holds
  // (`RegisterRouteError::Conflict`). Either failure leaves the directory.
  action registerSubscriberRoute(c: str): bool = all {
    s.subscribe.get(c) == SRouting,
    val left = { ...s, subscribers: s.subscribers.exclude(Set(c)) }
    if (s.subscriberRoute)
      s' = { ...left, subscribe: s.subscribe.set(c, SRejected) }
    else any {
      s' = { ...s, subscriberRoute: true, subscribe: s.subscribe.set(c, SSubscribed) },
      s' = { ...left, subscribe: s.subscribe.set(c, SFailed) },
    },
  }

  // `unregister_subscribe_namespace`; no client left withdraws the route.
  action receiveUnsubscribeNamespace(c: str): bool = all {
    not(busy(c)),
    s.subscribe.get(c) == SSubscribed,
    val remaining = s.subscribers.exclude(Set(c))
    s' = {
      ...s,
      subscribers: remaining,
      subscribe: s.subscribe.set(c, if (remaining == Set()) SWithdrawing else SIdle),
    },
  }

  // `cleanup_empty_namespace_subscription`
  action unregisterSubscriberRoute(c: str): bool = all {
    s.subscribe.get(c) == SWithdrawing,
    s' = { ...s, subscriberRoute: false, subscribe: s.subscribe.set(c, SIdle) },
  }

  // --- Session cleanup ---------------------------------------------------

  // `remove_session`, between requests of the session.
  action disconnect(c: str): bool = all {
    not(busy(c)),
    val publishers = s.publishers.exclude(Set(c))
    val subscribers = s.subscribers.exclude(Set(c))
    s' = {
      ...s,
      publishers: publishers,
      subscribers: subscribers,
      publish: s.publish.set(c, PIdle),
      subscribe: s.subscribe.set(c, SIdle),
      cleanup: s.cleanup.set(c, CPrefix),
      cleanupWithdrawsNamespace:
        if (s.publishers.contains(c) and publishers == Set())
          s.cleanupWithdrawsNamespace.union(Set(c))
        else s.cleanupWithdrawsNamespace,
      cleanupWithdrawsPrefix:
        if (s.subscribers.contains(c) and subscribers == Set())
          s.cleanupWithdrawsPrefix.union(Set(c))
        else s.cleanupWithdrawsPrefix,
    },
  }

  action cleanupPrefix(c: str): bool = all {
    s.cleanup.get(c) == CPrefix,
    s' = {
      ...s,
      subscriberRoute: if (s.cleanupWithdrawsPrefix.contains(c)) false else s.subscriberRoute,
      cleanupWithdrawsPrefix: s.cleanupWithdrawsPrefix.exclude(Set(c)),
      cleanup: s.cleanup.set(c, CNamespace),
    },
  }

  action cleanupNamespace(c: str): bool = all {
    s.cleanup.get(c) == CNamespace,
    nondet registered = oneOf(Set(true, false))
    s' = {
      ...s,
      publisherRoute:
        if (s.cleanupWithdrawsNamespace.contains(c))
          reconcilePublisher(s.publishers, s.publisherRoute, registered).route
        else s.publisherRoute,
      cleanupWithdrawsNamespace: s.cleanupWithdrawsNamespace.exclude(Set(c)),
      cleanup: s.cleanup.set(c, CNone),
    },
  }

  action step = {
    nondet c = oneOf(CLIENTS)
    any {
      receivePublishNamespace(c),
      reconcilePublisherRoute(c),
      rollBackPublisher(c),
      receivePublishNamespaceDone(c),
      receiveSubscribeNamespace(c),
      registerSubscriberRoute(c),
      receiveUnsubscribeNamespace(c),
      unregisterSubscriberRoute(c),
      disconnect(c),
      cleanupPrefix(c),
      cleanupNamespace(c),
    }
  }

  // --- Properties --------------------------------------------------------

  // A namespace a client was answered PUBLISH_NAMESPACE_OK for has its
  // route, so clients of other relays can subscribe to it.
  val publishedNamespaceHasRoute: bool =
    CLIENTS.exists(c => s.publish.get(c) == PPublished) implies s.publisherRoute

  // A prefix a client is subscribed to has its route once no first client
  // is still registering it.
  val subscribedPrefixHasRoute: bool =
    (CLIENTS.exists(c => s.subscribe.get(c) == SSubscribed) and
     CLIENTS.forall(c => s.subscribe.get(c) != SRouting))
      implies s.subscriberRoute

  // A single relay never overlaps itself: NAMESPACE_PREFIX_OVERLAP from the
  // route registry only means a route this relay was withdrawing.
  val noPrefixOverlapWithItself: bool =
    CLIENTS.forall(c => s.subscribe.get(c) != SRejected)

  // With every session idle, the publisher route is held exactly while a
  // client publishes the namespace, and no subscriber route is left behind.
  val idleRelayOwnsNoRoute: bool =
    CLIENTS.forall(c => not(busy(c))) implies and {
      s.publisherRoute == (s.publishers != Set()),
      s.subscriberRoute implies s.subscribers != Set(),
    }

  val safety: bool = and {
    publishedNamespaceHasRoute,
    subscribedPrefixHasRoute,
    noPrefixOverlapWithItself,
    idleRelayOwnsNoRoute,
  }
}

module relay_namespace {
  import namespace(CLIENTS = Set("a", "b", "c")).*
}

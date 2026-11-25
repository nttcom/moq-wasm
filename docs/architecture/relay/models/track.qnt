// A model of one track's entry in the relay's directory
// (services/relay/src/modules/directory/track.rs and directory.rs) and the
// sequences that change it: control_plane/sequences/subscribe.rs and
// subscribe/pending_producer_task.rs, publish.rs, publish_namespace.rs
// (`join_tracks_wanting_publishers` and
// `join_tracks_wanting_publisher_relays`), unsubscribe.rs,
// upstream_publish_done.rs, session_workers/session_cleanup.rs and
// control_plane/upstream_publisher_resolver.rs.
//
// Publishers announce the track's namespace, PUBLISH the track and answer
// the relay's upstream SUBSCRIBEs; subscribers SUBSCRIBE and UNSUBSCRIBE.
// Each session handles its requests one at a time, and every upstream
// SUBSCRIBE runs in its own `PendingProducerTask`. Each action is one step
// between two awaits: one directory operation, which holds the DashMap entry
// of the track, or a message exchange with a peer.
//
// A session in RELAYS is a remote relay (`SessionPeer::Relay`); the others
// are clients. The model is this relay's view: a remote relay is a session
// whose requests arrive in any order, standing for whatever its own clients
// do. Each relay dials the sessions it sends requests on
// (`InterRelayConnectionManager`), so a remote relay holds one session that
// publishes to this relay and another that subscribes from it, and a name in
// both PUBLISHERS and SUBSCRIBERS is those two sessions. As a publisher it
// is listed in the route registry while announced, may forward its
// PUBLISH_NAMESPACE (joining the tracks wanting relays), answers the upstream
// SUBSCRIBEs from its own clients (so with OK or an error) and never sends
// PUBLISH (`offers_track` gives relays no tracks); its session, once closed,
// is not dialled again. As a subscriber it SUBSCRIBEs and UNSUBSCRIBEs at
// any time, as its clients come and go.
//
// FETCH, SUBSCRIBE_UPDATE, the Forward State, the cache, the track route in
// the route registry, malformed tracks and the drain are left out.
//
// Simulate: npx @informalsystems/quint run --main relay_track --invariant safety track.qnt
// Check every run up to N steps (Apalache, Java 21+):
//   npx @informalsystems/quint verify --main relay_track --invariant safety --max-steps N track.qnt
module track {
  const PUBLISHERS: Set[str]
  const SUBSCRIBERS: Set[str]
  // The sessions of remote relays, among the publishers and subscribers.
  const RELAYS: Set[str]
  // `UPSTREAM_ATTEMPTS` in subscribe.rs.
  pure val UPSTREAM_ATTEMPTS = 3

  // The track's `CreationState`, a watch channel per track entry.
  type Creation = Creating | Created | Failed

  type Origin = BySubscribe | ByPublish

  // An upstream subscription: the publisher and the relay's Request ID.
  type Upstream = { pub: str, request: int }

  type Producer = { pub: str, request: int, origin: Origin }

  // `Reservations`.
  type Reservations = { client: int, relay: int }

  // `Track`; `live` is false while the directory has no entry for the key.
  type Track = {
    live: bool,
    gen: int,
    producers: Set[Producer],
    consumers: Set[str],
    pending: int,              // `pending_producers`
    subscribing: Set[str],     // `subscribing`
    lastProducerEnd: bool,     // `last_producer_end.is_some()`
    reservations: Reservations,
  }

  // A SUBSCRIBE's progress through `Subscribe::handle`. Every phase but
  // Idle and Subscribed holds a `TrackReservation` on the entry `gen`.
  type SubPhase =
    | Idle
    | Resolving    // `TrackLookup::Started`: resolves the publishers next
    | Dispatching  // resolved `found`; sends the upstream SUBSCRIBEs next
    | Waiting      // waits for the creation of the entry
    | Registering  // found the track fed; registers its consumer next
    | Accepting    // consumer registered; starts the egress and answers OK next
    | Subscribed   // answered SUBSCRIBE_OK, not told PUBLISH_DONE

  type Sub = { phase: SubPhase, gen: int, found: Set[str], attempts: int }

  // A `PendingProducerTask`'s progress.
  type TaskPhase =
    | Begin        // `begin_upstream_subscribe` next
    | Subscribing  // SUBSCRIBE sent to the publisher
    | Answered     // SUBSCRIBE_OK received
    | Ingesting    // the track takes the publisher; `start_ingest` next
    | Joined       // producer added; `end_if_publisher_gone` next

  type Task = { id: int, gen: int, pub: str, phase: TaskPhase }

  type State = {
    track: Track,
    creations: int -> Creation,
    nextGen: int,
    nextId: int,
    subs: str -> Sub,
    tasks: Set[Task],
    // Publisher sessions in the session repository.
    connected: Set[str],
    // Publishers that left the repository and whose `remove_session` is next.
    cleaningUp: Set[str],
    announced: Set[str],
    // Answered PUBLISH_NAMESPACE_OK; `join_tracks_wanting_publishers` next.
    joining: Set[str],
    // Sent PUBLISH; the relay registers the producer next.
    publishing: Set[str],
    // The subscriptions each publisher serves the relay.
    alive: Set[Upstream],
    // A subscriber was told PUBLISH_DONE while an upstream SUBSCRIBE could
    // still feed the track.
    doneWhileAwaitingProducer: bool,
  }

  var st: State

  pure val noTrack: Track = {
    live: false, gen: -1, producers: Set(), consumers: Set(), pending: 0,
    subscribing: Set(), lastProducerEnd: false, reservations: { client: 0, relay: 0 },
  }

  pure val idleSub: Sub = { phase: Idle, gen: -1, found: Set(), attempts: 0 }

  action init = st' = {
    track: noTrack,
    creations: Map(),
    nextGen: 0,
    nextId: 0,
    subs: SUBSCRIBERS.mapBy(_ => idleSub),
    tasks: Set(),
    connected: PUBLISHERS,
    cleaningUp: Set(),
    announced: Set(),
    joining: Set(),
    publishing: Set(),
    alive: Set(),
    doneWhileAwaitingProducer: false,
  }

  // --- Track helpers -----------------------------------------------------

  pure def isLive(t: Track, g: int): bool = t.live and t.gen == g

  pure def feeders(t: Track): Set[str] = t.producers.map(p => p.pub)

  pure def upstreamOf(p: Producer): Upstream = { pub: p.pub, request: p.request }

  pure def isRelay(session: str): bool = RELAYS.contains(session)

  pure def max0(n: int): int = if (n < 0) 0 else n

  // `Track::downstream_remains`.
  pure def downstreamRemains(t: Track): bool =
    t.consumers != Set() or t.reservations.client + t.reservations.relay > 0

  // `Track::client_remains`.
  pure def clientRemains(t: Track): bool =
    t.consumers.exists(x => not(isRelay(x))) or t.reservations.client > 0

  // `Track::wants`: a relay publisher only while a client watches the track.
  pure def wants(t: Track, publisher: str): bool =
    if (isRelay(publisher)) clientRemains(t) else downstreamRemains(t)

  // `Track::reserve` and `Track::release_reservation`.
  pure def reserve(t: Track, x: str, n: int): Track =
    val r = t.reservations
    {
      ...t,
      reservations:
        if (isRelay(x)) { ...r, relay: max0(r.relay + n) } else { ...r, client: max0(r.client + n) },
    }

  // `Track::takes`.
  pure def takes(state: State, publisher: str): bool =
    val t = state.track
    val isFirstCreation = state.creations.get(t.gen) == Creating and not(t.lastProducerEnd)
    not(feeders(t).contains(publisher)) and (isFirstCreation or wants(t, publisher))

  pure def markCreated(state: State): State =
    if (state.creations.get(state.track.gen) == Creating)
      { ...state, creations: state.creations.set(state.track.gen, Created) }
    else state

  // Removes the entry and ends its consumers with PUBLISH_DONE
  // (`end_downstream_subscriptions`).
  pure def endTrack(state: State): State =
    val t = state.track
    {
      ...state,
      track: noTrack,
      subs: state.subs.keys().mapBy(x =>
        val sub = state.subs.get(x)
        if (sub.phase == Subscribed and sub.gen == t.gen and t.consumers.contains(x)) idleSub
        else sub),
    }

  // `InMemoryLocalPubSubDirectory::end_publisher_subscription`.
  pure def endPublisherSubscription(state: State, publisher: str): State =
    val t = state.track
    if (not(t.live) or not(feeders(t).contains(publisher))) state
    else
      val left = { ...t, producers: t.producers.filter(p => p.pub != publisher) }
      if (left.producers != Set()) { ...state, track: left }
      // `Track::outlive_last_producer`.
      else if (left.pending > 0)
        {
          ...state,
          track: { ...left, lastProducerEnd: true },
          creations: state.creations.set(t.gen, Creating),
        }
      else endTrack({ ...state, track: left })

  // `InMemoryLocalPubSubDirectory::pending_producer_ended`.
  pure def pendingProducerEnded(state: State, g: int): State =
    val t = state.track
    if (not(isLive(t, g))) state
    else
      val left = { ...t, pending: max0(t.pending - 1) }
      if (left.pending > 0 or left.producers != Set()) { ...state, track: left }
      else
        val failed = { ...state, track: left, creations: state.creations.set(g, Failed) }
        if (left.lastProducerEnd) endTrack(failed) else { ...failed, track: noTrack }

  // The `TrackReservation` of subscriber `x` drops.
  pure def releaseReservation(state: State, g: int, x: str): State =
    val t = state.track
    if (isLive(t, g)) { ...state, track: reserve(t, x, -1) } else state

  // `track_or_begin_creation` for subscriber `x`.
  pure def lookup(state: State, x: str, attempts: int): State =
    val t = state.track
    if (t.live)
      val phase = if (t.producers == Set()) Waiting else Registering
      {
        ...state,
        track: reserve(t, x, 1),
        subs: state.subs.set(x, { phase: phase, gen: t.gen, found: Set(), attempts: attempts }),
      }
    else
      val g = state.nextGen
      {
        ...state,
        track: reserve({ ...noTrack, live: true, gen: g }, x, 1),
        creations: state.creations.put(g, Creating),
        nextGen: g + 1,
        subs: state.subs.set(x, { phase: Resolving, gen: g, found: Set(), attempts: attempts }),
      }

  // `Acceptance::UpstreamGone`: the reservation goes, and the SUBSCRIBE
  // looks the track up again or gives up with SUBSCRIBE_ERROR.
  pure def upstreamGone(state: State, x: str): State =
    val sub = state.subs.get(x)
    val released = releaseReservation(state, sub.gen, x)
    if (sub.attempts + 1 < UPSTREAM_ATTEMPTS) lookup(released, x, sub.attempts + 1)
    else { ...released, subs: released.subs.set(x, idleSub) }

  // `subscribe_publishers`.
  pure def spawnTasks(state: State, g: int, publishers: Set[str]): State =
    val t = state.track
    {
      ...state,
      track: if (isLive(t, g)) { ...t, pending: t.pending + publishers.size() } else t,
      tasks: state.tasks.union(publishers.map(p => { id: state.nextId, gen: g, pub: p, phase: Begin })),
      nextId: state.nextId + 1,
    }

  // `end_downstream_subscription` of consumer `x` without an end, then the
  // UNSUBSCRIBEs of the upstream subscriptions it released: once no client
  // remains, the SUBSCRIBE-initiated ones from relays, and once no one
  // remains, all SUBSCRIBE-initiated ones.
  pure def removeConsumer(state: State, x: str): State =
    val t = { ...state.track, consumers: state.track.consumers.exclude(Set(x)) }
    val released =
      if (clientRemains(t)) Set()
      else t.producers.filter(p => p.origin == BySubscribe and (not(downstreamRemains(t)) or isRelay(p.pub)))
    val left = { ...t, producers: t.producers.exclude(released) }
    val removed = { ...state, track: left, alive: state.alive.exclude(released.map(upstreamOf)) }
    if (left.producers == Set())
      {
        ...endTrack(removed),
        doneWhileAwaitingProducer:
          state.doneWhileAwaitingProducer or (left.pending > 0 and left.consumers != Set()),
      }
    else removed

  // --- SUBSCRIBE / UNSUBSCRIBE -------------------------------------------

  action subscribe(x: str): bool = all {
    st.subs.get(x).phase == Idle,
    st' = lookup(st, x, 0),
  }

  // `UpstreamPublisherResolver::resolve`: the namespace's client publishers
  // and the track's client PUBLISHers (`find_upstream_publishers`), and for
  // a client the relays the route registry lists too.
  action resolve(x: str): bool = all {
    st.subs.get(x).phase == Resolving,
    val local = st.announced.filter(p => not(isRelay(p))).union(
      if (st.track.live)
        st.track.producers.filter(p => p.origin == ByPublish and not(isRelay(p.pub))).map(p => p.pub)
      else Set())
    val found = if (isRelay(x)) local else local.union(st.announced.filter(p => isRelay(p)))
    st' = { ...st, subs: st.subs.set(x, { ...st.subs.get(x), phase: Dispatching, found: found }) },
  }

  action dispatch(x: str): bool = all {
    val sub = st.subs.get(x)
    all {
      sub.phase == Dispatching,
      val waiting = { ...st, subs: st.subs.set(x, { ...sub, phase: Waiting, found: Set() }) }
      st' =
        if (sub.found == Set()) pendingProducerEnded(waiting, sub.gen)
        else spawnTasks(waiting, sub.gen, sub.found),
    },
  }

  // `receiver.wait_for(..)` and `entry_state`: a failed creation answers
  // SUBSCRIBE_ERROR; an entry gone or created without a producer is looked
  // up again.
  action wait(x: str): bool = all {
    val sub = st.subs.get(x)
    val creation = st.creations.get(sub.gen)
    val live = isLive(st.track, sub.gen)
    all {
      sub.phase == Waiting,
      creation != Creating or not(live),
      st' =
        if (creation == Failed) { ...st, subs: st.subs.set(x, idleSub) }
        else if (live and st.track.producers != Set())
          { ...st, subs: st.subs.set(x, { ...sub, phase: Registering }) }
        else lookup(releaseReservation(st, sub.gen, x), x, sub.attempts),
    },
  }

  // `register_downstream_subscription`.
  action register(x: str): bool = all {
    val sub = st.subs.get(x)
    all {
      sub.phase == Registering,
      st' =
        if (isLive(st.track, sub.gen))
          {
            ...st,
            track: { ...st.track, consumers: st.track.consumers.union(Set(x)) },
            subs: st.subs.set(x, { ...sub, phase: Accepting }),
          }
        else upstreamGone(st, x),
    },
  }

  // `start_egress`, the egress runner's readiness and SUBSCRIBE_OK; then the
  // reservation goes.
  action accept(x: str): bool = all {
    val sub = st.subs.get(x)
    all {
      sub.phase == Accepting,
      st' =
        if (isLive(st.track, sub.gen) and st.track.consumers.contains(x))
          {
            ...releaseReservation(st, sub.gen, x),
            subs: st.subs.set(x, { ...sub, phase: Subscribed, found: Set() }),
          }
        else upstreamGone(st, x),
    },
  }

  action unsubscribe(x: str): bool = all {
    st.subs.get(x).phase == Subscribed,
    val removed = removeConsumer(st, x)
    st' = { ...removed, subs: removed.subs.set(x, idleSub) },
  }

  // A SUBSCRIBE that found the track fed is answered SUBSCRIBE_ERROR before
  // registering (`handler.understands` refuses the track properties), or,
  // registered, its egress cannot start or its SUBSCRIBE_OK cannot be sent
  // and `remove_downstream_subscription` removes its consumer while its
  // reservation is still held; the reservation drops after either.
  action refuse(x: str): bool = all {
    val sub = st.subs.get(x)
    all {
      Set(Registering, Accepting).contains(sub.phase),
      val removed =
        if (sub.phase == Accepting and isLive(st.track, sub.gen) and st.track.consumers.contains(x))
          removeConsumer(st, x)
        else st
      val released = releaseReservation(removed, sub.gen, x)
      st' = { ...released, subs: released.subs.set(x, idleSub) },
    },
  }

  // --- Upstream SUBSCRIBE (`PendingProducerTask`) ------------------------

  pure def withTask(state: State, t: Task, phase: TaskPhase): State =
    { ...state, tasks: state.tasks.exclude(Set(t)).union(Set({ ...t, phase: phase })) }

  // The task ends: `end_upstream_subscribe` when it had begun, then
  // `pending_producer_ended`.
  pure def finish(state: State, t: Task, begun: bool): State =
    val track = state.track
    val ended =
      if (begun and isLive(track, t.gen))
        { ...state, track: { ...track, subscribing: track.subscribing.exclude(Set(t.pub)) } }
      else state
    pendingProducerEnded({ ...ended, tasks: ended.tasks.exclude(Set(t)) }, t.gen)

  pure def unsubscribeUpstream(state: State, t: Task): State =
    { ...state, alive: state.alive.exclude(Set({ pub: t.pub, request: t.id })) }

  // `session_of` (a relay whose session closed is not reached), then
  // `begin_upstream_subscribe`.
  action taskBegin(t: Task): bool = all {
    t.phase == Begin,
    val track = st.track
    val reached = not(isRelay(t.pub)) or st.connected.contains(t.pub)
    st' =
      if (not(reached)) finish(st, t, false)
      else if (isLive(track, t.gen) and not(feeders(track).contains(t.pub)) and not(track.subscribing.contains(t.pub)))
        withTask({ ...st, track: { ...track, subscribing: track.subscribing.union(Set(t.pub)) } }, t, Subscribing)
      else finish(st, t, false),
  }

  // The publisher answers SUBSCRIBE_OK or SUBSCRIBE_ERROR; a session that
  // left answers nothing and the SUBSCRIBE fails.
  action taskAnswer(t: Task): bool = all {
    t.phase == Subscribing,
    nondet accepted = oneOf(Set(true, false))
    st' =
      if (accepted and st.connected.contains(t.pub))
        withTask({ ...st, alive: st.alive.union(Set({ pub: t.pub, request: t.id })) }, t, Answered)
      else finish(st, t, true),
  }

  // `published_upstream_subscription` (by key, whatever the entry) and
  // `track_takes_publisher`.
  action taskAdmit(t: Task): bool = all {
    t.phase == Answered,
    val published = st.track.live and st.track.producers.exists(p => p.pub == t.pub and p.origin == ByPublish)
    st' =
      if (published or not(isLive(st.track, t.gen) and takes(st, t.pub)))
        finish(unsubscribeUpstream(st, t), t, true)
      else withTask(st, t, Ingesting),
  }

  // `start_ingest`, then `add_upstream_subscription_to_track`.
  action taskIngest(t: Task): bool = all {
    t.phase == Ingesting,
    st' =
      if (not(st.connected.contains(t.pub))) finish(st, t, true)
      else if (isLive(st.track, t.gen) and takes(st, t.pub))
        val added = {
          ...st,
          track: { ...st.track, producers: st.track.producers.union(Set({ pub: t.pub, request: t.id, origin: BySubscribe })) },
        }
        withTask(markCreated(added), t, Joined)
      else finish(unsubscribeUpstream(st, t), t, true),
  }

  // `end_if_publisher_gone`.
  action taskCheckGone(t: Task): bool = all {
    t.phase == Joined,
    val track = st.track
    val gone = not(st.connected.contains(t.pub))
    val ownProducer = track.live and track.producers.contains({ pub: t.pub, request: t.id, origin: BySubscribe })
    st' =
      if (gone and ownProducer) finish(endPublisherSubscription(st, t.pub), t, true)
      else finish(st, t, true),
  }

  // --- Publishers --------------------------------------------------------

  // PUBLISH_NAMESPACE: the directory records it, then the route and the OK.
  // A relay is in the route registry once its client announced; its
  // PUBLISH_NAMESPACE reaches this relay only when this relay subscribed to
  // the namespace there.
  action announce(p: str): bool = all {
    st.connected.contains(p),
    not(st.announced.contains(p)),
    not(st.joining.contains(p)),
    nondet forwarded = oneOf(Set(true, false))
    st' = {
      ...st,
      announced: st.announced.union(Set(p)),
      joining: if (forwarded or not(isRelay(p))) st.joining.union(Set(p)) else st.joining,
    },
  }

  // `join_tracks_wanting_publishers` for a client, which subscribes it;
  // `join_tracks_wanting_publisher_relays` for a relay, which subscribes
  // every relay the route registry lists. Both read
  // `tracks_wanting_publisher`, then call `subscribe_publishers`.
  action join(p: str): bool = all {
    st.joining.contains(p),
    val track = st.track
    val joined = { ...st, joining: st.joining.exclude(Set(p)) }
    val relays = st.announced.filter(r => isRelay(r))
    st' =
      if (not(track.live) or not(wants(track, p))) joined
      else if (not(isRelay(p)))
        if (feeders(track).contains(p)) joined else spawnTasks(joined, track.gen, Set(p))
      else if (relays == Set()) joined
      else spawnTasks(joined, track.gen, relays),
  }

  action unannounce(p: str): bool = all {
    st.connected.contains(p),
    st.announced.contains(p),
    not(st.joining.contains(p)),
    st' = { ...st, announced: st.announced.exclude(Set(p)) },
  }

  action publish(p: str): bool = all {
    not(isRelay(p)),
    st.connected.contains(p),
    not(st.publishing.contains(p)),
    not(st.track.producers.exists(pr => pr.pub == p and pr.origin == ByPublish)),
    st' = { ...st, publishing: st.publishing.union(Set(p)) },
  }

  // `start_ingest`, then `register_published_subscription`, which replaces
  // the publisher's SUBSCRIBE-initiated producer and unsubscribes it.
  action publishRegister(p: str): bool = all {
    st.publishing.contains(p),
    val done = { ...st, publishing: st.publishing.exclude(Set(p)) }
    st' =
      if (not(st.connected.contains(p))) done
      else
        val t = if (done.track.live) done.track else { ...noTrack, live: true, gen: done.nextGen }
        val replaced = t.producers.filter(pr => pr.pub == p)
        val producer = { pub: p, request: done.nextId, origin: ByPublish }
        val registered = {
          ...done,
          track: { ...t, producers: t.producers.exclude(replaced).union(Set(producer)) },
          creations: if (done.track.live) done.creations else done.creations.put(t.gen, Created),
          nextGen: if (done.track.live) done.nextGen else done.nextGen + 1,
          nextId: done.nextId + 1,
          alive: done.alive.exclude(replaced.map(upstreamOf)).union(Set(upstreamOf(producer))),
        }
        markCreated(registered),
  }

  // The publisher sends PUBLISH_DONE for one of its subscriptions; the relay
  // ends it (`UpstreamPublishDone`, `end_upstream_subscription`).
  action publishDone(p: str): bool = all {
    st.connected.contains(p),
    val own = st.alive.filter(u => u.pub == p)
    all {
      own != Set(),
      nondet u = oneOf(own)
      val sent = { ...st, alive: st.alive.exclude(Set(u)) }
      st' =
        if (st.track.live and st.track.producers.exists(pr => upstreamOf(pr) == u))
          endPublisherSubscription(sent, p)
        else sent,
    },
  }

  // `cleanup_session`: the session leaves the repository, ending every
  // subscription it served ...
  action disconnect(p: str): bool = all {
    st.connected.contains(p),
    st' = {
      ...st,
      connected: st.connected.exclude(Set(p)),
      cleaningUp: st.cleaningUp.union(Set(p)),
      alive: st.alive.filter(u => u.pub != p),
    },
  }

  // ... then `remove_session` drops it from the directory.
  action cleanUp(p: str): bool = all {
    st.cleaningUp.contains(p),
    val removed = { ...st, cleaningUp: st.cleaningUp.exclude(Set(p)), announced: st.announced.exclude(Set(p)) }
    st' = endPublisherSubscription(removed, p),
  }

  action step = any {
    nondet x = oneOf(SUBSCRIBERS)
    any { subscribe(x), resolve(x), dispatch(x), wait(x), register(x), accept(x), unsubscribe(x), refuse(x) },
    nondet p = oneOf(PUBLISHERS)
    any { announce(p), join(p), unannounce(p), publish(p), publishRegister(p), publishDone(p), disconnect(p), cleanUp(p) },
    all {
      st.tasks != Set(),
      nondet t = oneOf(st.tasks)
      any { taskBegin(t), taskAnswer(t), taskAdmit(t), taskIngest(t), taskCheckGone(t) },
    },
  }

  // --- Properties --------------------------------------------------------

  pure val reserving = Set(Resolving, Dispatching, Waiting, Registering, Accepting)

  // Nothing is in flight: every SUBSCRIBE is answered and every task and
  // publisher request is done.
  val quiescent: bool = and {
    st.tasks == Set(),
    st.joining == Set(),
    st.publishing == Set(),
    st.cleaningUp == Set(),
    SUBSCRIBERS.forall(x => Set(Idle, Subscribed).contains(st.subs.get(x).phase)),
  }

  // A subscriber answered SUBSCRIBE_OK and not told PUBLISH_DONE is a
  // consumer of the track.
  val subscribedIsConsumer: bool =
    SUBSCRIBERS.forall(x =>
      val sub = st.subs.get(x)
      sub.phase == Subscribed implies (isLive(st.track, sub.gen) and st.track.consumers.contains(x)))

  // An entry without a producer is being created. A SUBSCRIBE finding a
  // created entry without one would look it up again forever.
  val unfedTrackIsCreating: bool =
    (st.track.live and st.track.producers == Set()) implies st.creations.get(st.track.gen) == Creating

  // A track being created has something that will end the creation: a
  // creator that has not sent its upstream SUBSCRIBEs yet or an upstream
  // SUBSCRIBE in flight. Otherwise its consumers and the SUBSCRIBEs waiting
  // for it wait forever.
  val noStuckCreation: bool =
    (st.track.live and st.creations.get(st.track.gen) == Creating) implies or {
      st.tasks.exists(t => t.gen == st.track.gen),
      SUBSCRIBERS.exists(x =>
        val sub = st.subs.get(x)
        Set(Resolving, Dispatching).contains(sub.phase) and sub.gen == st.track.gen),
    }

  // Every producer reads a subscription its publisher still serves, or one
  // of a publisher whose `remove_session` is next.
  val producersAreServed: bool =
    st.track.producers.forall(p => st.alive.contains(upstreamOf(p)) or st.cleaningUp.contains(p.pub))

  // With nothing in flight, every subscription a publisher serves feeds the
  // track, and a SUBSCRIBE-initiated one only while someone receives it.
  val quiescentUpstreamsAreNeeded: bool =
    quiescent implies and {
      st.alive.forall(u => st.track.live and st.track.producers.exists(p => upstreamOf(p) == u)),
      st.track.producers.forall(p => p.origin == BySubscribe implies st.track.consumers != Set()),
    }

  // Each peer's reservation count is the number of its SUBSCRIBEs holding one.
  val reservationsMatch: bool =
    val holders = SUBSCRIBERS.filter(x =>
      val sub = st.subs.get(x)
      reserving.contains(sub.phase) and sub.gen == st.track.gen)
    st.track.live implies and {
      st.track.reservations.client == holders.filter(x => not(isRelay(x))).size(),
      st.track.reservations.relay == holders.filter(x => isRelay(x)).size(),
    }

  // A creation fails only once no upstream SUBSCRIBE for it is in flight:
  // the SUBSCRIBEs waiting for it are answered with the last failure, not
  // before a publisher could answer.
  val creationFailsLast: bool =
    st.tasks.forall(t => t.phase == Subscribing implies st.creations.get(t.gen) != Failed)

  // `outlive_last_producer`: consumers wait for an upstream SUBSCRIBE in
  // flight instead of ending.
  val noDoneWhileAwaitingProducer: bool = not(st.doneWhileAwaitingProducer)

  // Split horizon: a relay subscriber is served from relay publishers alone
  // only while a client wants the track, so two relays feeding each other
  // never keep a track alive between them.
  val splitHorizon: bool =
    val t = st.track
    val relayServedByRelays = and {
      t.consumers.exists(x => isRelay(x)),
      t.producers != Set(),
      t.producers.forall(p => isRelay(p.pub)),
    }
    relayServedByRelays implies clientRemains(t)

  // With nothing in flight, a relay feeds the track by SUBSCRIBE only while
  // a client receives it.
  val quiescentRelayUpstreamsServeClients: bool =
    quiescent implies st.track.producers.forall(p =>
      (p.origin == BySubscribe and isRelay(p.pub)) implies st.track.consumers.exists(x => not(isRelay(x))))

  val safety: bool = and {
    subscribedIsConsumer,
    unfedTrackIsCreating,
    noStuckCreation,
    producersAreServed,
    quiescentUpstreamsAreNeeded,
    reservationsMatch,
    creationFailsLast,
    noDoneWhileAwaitingProducer,
    splitHorizon,
    quiescentRelayUpstreamsServeClients,
  }
}

// Two client publishers, two client subscribers and one remote relay that
// both publishes the track to this relay and subscribes to it.
module relay_track {
  import track(
    PUBLISHERS = Set("p1", "p2", "r1"),
    SUBSCRIBERS = Set("s1", "s2", "r1"),
    RELAYS = Set("r1"),
  ).*
}

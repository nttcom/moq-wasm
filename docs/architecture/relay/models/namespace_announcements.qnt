// A model of what each SUBSCRIBE_NAMESPACE subscriber of one relay is told:
// the namespaces in the directory (services/relay/src/modules/directory.rs),
// the sequences that announce and withdraw them
// (control_plane/sequences/subscribe_namespace.rs, publish_namespace.rs,
// publish_namespace_done.rs, unsubscribe_namespace.rs and
// session_workers/session_cleanup.rs), the namespaces
// `ControlMessageForwarder` records as announced on each session
// (session/session_repository.rs) and what moqt's draft-18 session makes of
// each NAMESPACE and NAMESPACE_DONE (crates/moqt/src/modules/moqt/version/
// draft18.rs: `advertise_namespace`, `accept_namespace_subscription`,
// `move_namespace_subscription` and the `PublishNamespaceDone`
// notification).
//
// Client publishers send PUBLISH_NAMESPACE, PUBLISH_NAMESPACE_DONE (naming
// its request on draft-18, all of the session's publications of the
// namespace on draft-14) and disconnect; a publication is keyed by (session,
// Request ID), here by (publisher, slot). Client subscribers hold at most one
// SUBSCRIBE_NAMESPACE of namespaces, update its prefix and
// UNSUBSCRIBE_NAMESPACE. Each session handles its requests one at a time,
// and the sessions run concurrently. Each action is one step between two
// awaits: a directory operation, or the decision, record and send of one
// announcement or withdrawal on a subscriber's session, which
// `ControlMessageForwarder` holds the session's announcement lock across.
// The namespace routes (namespace.qnt) appear only as a reconcile that may
// fail. Publishers and
// subscribers are separate sessions: the relay tells a session of its own
// namespace like any other (`notify_local_subscribers`), so no subscriber is
// left out. Relay sessions, other relays, SUBSCRIBE_TRACKS and PUBLISH, a
// subscriber's disconnect and an update moqt answers TIMEOUT are left out.
//
// Simulate: npx @informalsystems/quint run --main relay_namespace_announcements --invariant safety namespace_announcements.qnt
module namespace_announcements {
  const PUBLISHERS: Set[str]
  const SUBSCRIBERS: Set[str]
  const NAMESPACES: Set[str]
  const PREFIXES: Set[str]
  // (prefix, namespace): the namespace lies under the prefix
  // (`namespace_has_prefix`).
  const UNDER: Set[(str, str)]
  // The Request IDs a publisher may hold publications of one namespace with.
  pure val SLOTS = Set(0, 1)

  pure def under(n: str, f: str): bool = UNDER.contains((f, n))

  type Publication = { pub: str, ns: str, slot: int }
  type Subscription = { sub: str, prefix: str }

  // moqt's view of the subscriber's SUBSCRIBE_NAMESPACE: unanswered with the
  // NAMESPACEs held for its REQUEST_OK, or answered with those advertised.
  type MoqtState = MNone | MHeld | MActive
  type MoqtSub = { state: MoqtState, prefix: str, held: Set[str], told: Set[str] }

  type Kind = Announcement | Withdrawal

  type PubPhase =
    | PIdle
    | PRouting        // PUBLISH_NAMESPACE in the directory; reconcile next
    | PNotifying      // `notify_to_subscribers`
    | PFailNotifying  // reconcile failed: `notify_local_subscribers`
    | PUnregistered   // PUBLISH_NAMESPACE_DONE left the directory
    | PWithdrawing    // `notify_local_subscribers`
    | PReannouncing   // a late DONE: the session still publishes it
    | PLeaving        // `cleanup_session`: namespaces left to withdraw
    | PLeaveNotifying // `notify_local_subscribers` of one of them

  // `todo` holds the subscribers the loop has yet to tell, `withdrawn`
  // `withdrawn_from`.
  type PubTask = {
    phase: PubPhase, ns: str, slot: int,
    todo: Set[str], withdrawn: Set[str], leaving: Set[str],
  }

  type SubPhase =
    | SIdle
    | SRouting      // SUBSCRIBE_NAMESPACE in the directory; reconcile next
    | SFailing      // reconcile failed and left the directory; error next
    | SOffering     // `forward_matching_publications`
    | URouting      // update: new prefix in the directory; reconcile next
    | UFailing      // update's reconcile failed; error next
    | UReleasing    // update answered; old prefix leaves the directory next
    | UForgetting   // `forget_announced_namespaces` next
    | UnReleasing   // UNSUBSCRIBE_NAMESPACE: leaves the directory next
    | UnForgetting  // `forget_announced_namespaces` next

  type SubTask = { phase: SubPhase, prefix: str, target: str, todo: Set[str] }

  type State = {
    pubs: Set[Publication],
    subs: Set[Subscription],
    // `SessionEntry::announced_namespaces`.
    rec: str -> Set[str],
    moqt: str -> MoqtSub,
    // NAMESPACEs no subscription of the session covered, which moqt sends as
    // a PUBLISH_NAMESPACE request of its own.
    stray: str -> Set[str],
    pubTasks: str -> PubTask,
    subTasks: str -> SubTask,
    duplicate: bool,
    untoldDone: bool,
    strayAnnounced: bool,
  }

  var s: State

  pure val idlePub: PubTask = {
    phase: PIdle, ns: "", slot: -1, todo: Set(), withdrawn: Set(), leaving: Set(),
  }
  pure val idleSub: SubTask = { phase: SIdle, prefix: "", target: "", todo: Set() }
  pure val noMoqtSub: MoqtSub = { state: MNone, prefix: "", held: Set(), told: Set() }

  action init = s' = {
    pubs: Set(),
    subs: Set(),
    rec: SUBSCRIBERS.mapBy(_ => Set()),
    moqt: SUBSCRIBERS.mapBy(_ => noMoqtSub),
    stray: SUBSCRIBERS.mapBy(_ => Set()),
    pubTasks: PUBLISHERS.mapBy(_ => idlePub),
    subTasks: SUBSCRIBERS.mapBy(_ => idleSub),
    duplicate: false,
    untoldDone: false,
    strayAnnounced: false,
  }

  // --- Directory --------------------------------------------------------

  pure def published(st: State, n: str): bool = st.pubs.exists(q => q.ns == n)

  // `get_namespace_subscribers`.
  pure def subscribersOf(st: State, n: str): Set[str] =
    st.subs.filter(sb => under(n, sb.prefix)).map(sb => sb.sub)

  // `notify_local_subscribers`: none while another session publishes it.
  pure def withdrawalTargets(st: State, n: str, p: str): Set[str] =
    if (st.pubs.exists(q => q.ns == n and q.pub != p)) Set() else subscribersOf(st, n)

  pure def publishedUnder(st: State, f: str): Set[str] =
    NAMESPACES.filter(n => under(n, f) and published(st, n))

  // --- ControlMessageForwarder ------------------------------------------

  // The record of `publish_namespace` / `publish_namespace_done`: the send
  // follows when `sent`. moqt's `begin_publish_subscribed_namespace` sends
  // nothing when no subscription of the session covers the namespace, and
  // the relay then takes the record back.
  pure def record(st: State, x: str, n: str, kind: Kind): { st: State, sent: bool } =
    val announced = st.rec.get(x).contains(n)
    val m = st.moqt.get(x)
    val covered = m.state != MNone and under(n, m.prefix)
    match kind {
      | Announcement =>
        if (announced or not(covered)) { st: st, sent: false }
        else { st: { ...st, rec: st.rec.set(x, st.rec.get(x).union(Set(n))) }, sent: true }
      | Withdrawal =>
        { st: { ...st, rec: st.rec.set(x, st.rec.get(x).exclude(Set(n))) }, sent: announced }
    }

  // What the subscriber's application was told, net: by NAMESPACE on its
  // subscription or by a PUBLISH_NAMESPACE request, both of which moqt raises
  // as `SessionEvent::PublishNamespace`.
  pure def toldTo(st: State, x: str): Set[str] = st.moqt.get(x).told.union(st.stray.get(x))

  // The send, as moqt's draft-18 session handles it: a NAMESPACE is
  // held until REQUEST_OK, swallowed when advertised, and sent as a
  // PUBLISH_NAMESPACE request when no subscription covers it; a DONE ends
  // whichever of them told of it, or is dropped.
  pure def deliver(st: State, x: str, n: str, kind: Kind): State =
    val m = st.moqt.get(x)
    val covered = m.state != MNone and under(n, m.prefix)
    match kind {
      | Announcement =>
        val flagged = { ...st, duplicate: st.duplicate or toldTo(st, x).union(m.held).contains(n) }
        if (not(covered))
          { ...flagged, stray: st.stray.set(x, st.stray.get(x).union(Set(n))), strayAnnounced: true }
        else if (m.state == MHeld)
          { ...flagged, moqt: st.moqt.set(x, { ...m, held: m.held.union(Set(n)) }) }
        else
          { ...flagged, moqt: st.moqt.set(x, { ...m, told: m.told.union(Set(n)) }) }
      | Withdrawal =>
        if (m.state == MHeld and m.held.contains(n))
          { ...st, moqt: st.moqt.set(x, { ...m, held: m.held.exclude(Set(n)) }) }
        else if (m.state == MActive and m.told.contains(n))
          { ...st, moqt: st.moqt.set(x, { ...m, told: m.told.exclude(Set(n)) }) }
        else if (st.stray.get(x).contains(n))
          { ...st, stray: st.stray.set(x, st.stray.get(x).exclude(Set(n))) }
        else { ...st, untoldDone: st.untoldDone or covered }
    }

  // --- Publishers -------------------------------------------------------

  pure val loopPhases = Set(PNotifying, PFailNotifying, PWithdrawing, PReannouncing, PLeaveNotifying)

  pure def setPub(st: State, p: str, t: PubTask): State = { ...st, pubTasks: st.pubTasks.set(p, t) }

  // The step that ends a loop also runs what follows it up to the next await.
  pure def advancePub(st: State, p: str): State =
    val t = st.pubTasks.get(p)
    if (loopPhases.contains(t.phase) and t.todo == Set())
      if (t.phase == PWithdrawing and t.withdrawn != Set())
        setPub(st, p, { ...t, phase: PReannouncing, todo: t.withdrawn })
      else if (t.phase == PLeaveNotifying)
        if (t.leaving == Set()) setPub(st, p, idlePub) else setPub(st, p, { ...t, phase: PLeaving })
      else setPub(st, p, idlePub)
    else st

  action announce(p: str): bool = all {
    s.pubTasks.get(p).phase == PIdle,
    nondet n = oneOf(NAMESPACES)
    nondet k = oneOf(SLOTS)
    val mine = { pub: p, ns: n, slot: k }
    all {
      not(s.pubs.contains(mine)),
      s' = setPub({ ...s, pubs: s.pubs.union(Set(mine)) }, p, { ...idlePub, phase: PRouting, ns: n, slot: k }),
    },
  }

  // The route registered, PUBLISH_NAMESPACE_OK, then `notify_to_subscribers`
  // reads the subscribers.
  action publishRouted(p: str): bool = all {
    val t = s.pubTasks.get(p)
    all {
      t.phase == PRouting,
      s' = advancePub(setPub(s, p, { ...t, phase: PNotifying, todo: subscribersOf(s, t.ns) }), p),
    },
  }

  // The reconcile fails only while no other publication of the namespace
  // holds its route.
  action publishRouteFailed(p: str): bool = all {
    val t = s.pubTasks.get(p)
    val mine = { pub: p, ns: t.ns, slot: t.slot }
    all {
      t.phase == PRouting,
      s.pubs.forall(q => (q.ns == t.ns and q != mine) implies
        val other = s.pubTasks.get(q.pub)
        other.phase == PRouting and other.ns == q.ns and other.slot == q.slot),
      val left = { ...s, pubs: s.pubs.exclude(Set(mine)) }
      s' = advancePub(setPub(left, p, { ...t, phase: PFailNotifying, todo: withdrawalTargets(left, t.ns, p) }), p),
    },
  }

  // draft-18 names the request; draft-14 (slot -1) ends all of the session's
  // publications of the namespace. A DONE of nothing published is ignored.
  action publishNamespaceDone(p: str): bool = all {
    s.pubTasks.get(p).phase == PIdle,
    nondet n = oneOf(NAMESPACES)
    nondet k = oneOf(SLOTS.union(Set(-1)))
    val ended = s.pubs.filter(q => q.pub == p and q.ns == n and (k == -1 or q.slot == k))
    all {
      ended != Set(),
      s' = setPub({ ...s, pubs: s.pubs.exclude(ended) }, p, { ...idlePub, phase: PUnregistered, ns: n, slot: k }),
    },
  }

  action withdrawalRead(p: str): bool = all {
    val t = s.pubTasks.get(p)
    all {
      t.phase == PUnregistered,
      s' = advancePub(setPub(s, p, { ...t, phase: PWithdrawing, todo: withdrawalTargets(s, t.ns, p) }), p),
    },
  }

  // `remove_session` drops every publication of the session.
  action disconnect(p: str): bool = all {
    s.pubTasks.get(p).phase == PIdle,
    val own = s.pubs.filter(q => q.pub == p)
    all {
      own != Set(),
      s' = setPub({ ...s, pubs: s.pubs.exclude(own) }, p, { ...idlePub, phase: PLeaving, leaving: own.map(q => q.ns) }),
    },
  }

  action leaveNext(p: str): bool = all {
    val t = s.pubTasks.get(p)
    all {
      t.phase == PLeaving,
      nondet n = oneOf(t.leaving)
      val task = { ...t, phase: PLeaveNotifying, ns: n, leaving: t.leaving.exclude(Set(n)), todo: withdrawalTargets(s, n, p) }
      s' = advancePub(setPub(s, p, task), p),
    },
  }

  // One subscriber of the loop: the decision, `record` and the send. An
  // announcement goes out while the namespace is published, a late DONE's
  // one while the session still publishes it, and a withdrawal while no
  // other session publishes it.
  action pubTell(p: str): bool = all {
    val t = s.pubTasks.get(p)
    all {
      loopPhases.contains(t.phase),
      t.todo != Set(),
      nondet x = oneOf(t.todo)
      val kind = if (Set(PNotifying, PReannouncing).contains(t.phase)) Announcement else Withdrawal
      val wanted =
        if (t.phase == PNotifying) published(s, t.ns)
        else if (t.phase == PReannouncing) s.pubs.exists(q => q.pub == p and q.ns == t.ns)
        else not(s.pubs.exists(q => q.ns == t.ns and q.pub != p))
      val r = if (wanted) record(s, x, t.ns, kind) else { st: s, sent: false }
      val task = {
        ...t,
        todo: t.todo.exclude(Set(x)),
        withdrawn: if (kind == Withdrawal and r.sent) t.withdrawn.union(Set(x)) else t.withdrawn,
      }
      s' = advancePub(setPub(if (r.sent) deliver(r.st, x, t.ns, kind) else r.st, p, task), p),
    },
  }

  // --- Subscribers ------------------------------------------------------

  pure def setSub(st: State, x: str, t: SubTask): State = { ...st, subTasks: st.subTasks.set(x, t) }

  pure def advanceSub(st: State, x: str): State =
    val t = st.subTasks.get(x)
    if (t.phase == SOffering and t.todo == Set()) setSub(st, x, idleSub) else st

  // A reconcile of the prefix fails only while no settled subscription of
  // it holds its route.
  pure def routeMayFail(st: State, x: str, f: str): bool =
    st.subs.forall(sb => (sb.prefix == f and sb.sub != x) implies
      val other = st.subTasks.get(sb.sub)
      (other.phase == SRouting and other.prefix == f) or (other.phase == URouting and other.target == f))

  action subscribeNamespace(x: str): bool = all {
    s.subTasks.get(x).phase == SIdle,
    s.moqt.get(x).state == MNone,
    nondet f = oneOf(PREFIXES)
    s' = setSub({
      ...s,
      moqt: s.moqt.set(x, { state: MHeld, prefix: f, held: Set(), told: Set() }),
      subs: s.subs.union(Set({ sub: x, prefix: f })),
    }, x, { ...idleSub, phase: SRouting, prefix: f }),
  }

  // The route registered, SUBSCRIBE_NAMESPACE_OK (moqt sends the held
  // NAMESPACEs), then `prefix_matches`.
  action subscribeRouted(x: str): bool = all {
    val t = s.subTasks.get(x)
    val m = s.moqt.get(x)
    all {
      t.phase == SRouting,
      val answered = { ...s, moqt: s.moqt.set(x, { ...m, state: MActive, held: Set(), told: m.held }) }
      s' = advanceSub(setSub(answered, x, { ...t, phase: SOffering, todo: publishedUnder(s, t.prefix) }), x),
    },
  }

  // `forget_refused_announcements` after the error.
  pure def forgetUnder(st: State, x: str, f: str): State =
    { ...st, rec: st.rec.set(x, st.rec.get(x).filter(n => not(under(n, f)))) }

  action subscribeRouteFailed(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == SRouting,
      routeMayFail(s, x, t.prefix),
      s' = setSub({ ...s, subs: s.subs.exclude(Set({ sub: x, prefix: t.prefix })) }, x, { ...t, phase: SFailing }),
    },
  }

  // SUBSCRIBE_NAMESPACE_ERROR: moqt drops the subscription with what it held.
  action subscribeRefused(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == SFailing,
      s' = setSub(forgetUnder({ ...s, moqt: s.moqt.set(x, noMoqtSub) }, x, t.prefix), x, idleSub),
    },
  }

  action updatePrefix(x: str): bool = all {
    s.subTasks.get(x).phase == SIdle,
    val m = s.moqt.get(x)
    all {
      m.state == MActive,
      nondet g = oneOf(PREFIXES.exclude(Set(m.prefix)))
      s' = setSub({ ...s, subs: s.subs.union(Set({ sub: x, prefix: g })) }, x,
        { ...idleSub, phase: URouting, prefix: m.prefix, target: g }),
    },
  }

  // The route registered, then `handler.ok()`: moqt moves the subscription
  // and forgets what the new prefix leaves out.
  action updateRouted(x: str): bool = all {
    val t = s.subTasks.get(x)
    val m = s.moqt.get(x)
    all {
      t.phase == URouting,
      val moved = { ...m, prefix: t.target, told: m.told.filter(n => under(n, t.target)) }
      s' = setSub({ ...s, moqt: s.moqt.set(x, moved) }, x, { ...t, phase: UReleasing }),
    },
  }

  action updateRouteFailed(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == URouting,
      routeMayFail(s, x, t.target),
      s' = setSub({ ...s, subs: s.subs.exclude(Set({ sub: x, prefix: t.target })) }, x, { ...t, phase: UFailing }),
    },
  }

  // A refused update ends the subscription; moqt reports it as
  // UNSUBSCRIBE_NAMESPACE of the old prefix, which the session handles next.
  action updateRefused(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == UFailing,
      val refused = forgetUnder({ ...s, moqt: s.moqt.set(x, noMoqtSub) }, x, t.target)
      s' = setSub(refused, x, { ...idleSub, phase: UnReleasing, prefix: t.prefix }),
    },
  }

  action updateReleased(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == UReleasing,
      s' = setSub({ ...s, subs: s.subs.exclude(Set({ sub: x, prefix: t.prefix })) }, x, { ...t, phase: UForgetting }),
    },
  }

  // `forget_announced_namespaces`, then `prefix_matches` less what the
  // old prefix covered (`Offer::was_offered`).
  action updateForgot(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == UForgetting,
      val kept = s.rec.get(x).filter(n => not(under(n, t.prefix)) or under(n, t.target))
      val offered = publishedUnder(s, t.target).filter(n => not(under(n, t.prefix)))
      s' = advanceSub(setSub({ ...s, rec: s.rec.set(x, kept) }, x,
        { ...t, phase: SOffering, prefix: t.target, target: "", todo: offered }), x),
    },
  }

  // The client cancels the subscription; moqt forgets what it advertised.
  action unsubscribeNamespace(x: str): bool = all {
    s.subTasks.get(x).phase == SIdle,
    val m = s.moqt.get(x)
    all {
      m.state == MActive,
      s' = setSub({ ...s, moqt: s.moqt.set(x, noMoqtSub) }, x, { ...idleSub, phase: UnReleasing, prefix: m.prefix }),
    },
  }

  action unsubscribeReleased(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == UnReleasing,
      s' = setSub({ ...s, subs: s.subs.exclude(Set({ sub: x, prefix: t.prefix })) }, x, { ...t, phase: UnForgetting }),
    },
  }

  action unsubscribeForgot(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == UnForgetting,
      s' = setSub({ ...s, rec: s.rec.set(x, s.rec.get(x).filter(n => not(under(n, t.prefix)))) }, x, idleSub),
    },
  }

  // `forward_matching_publications`: one PUBLISH_NAMESPACE, sent while
  // `announces_namespace_to` holds.
  action offerRecord(x: str): bool = all {
    val t = s.subTasks.get(x)
    all {
      t.phase == SOffering,
      t.todo != Set(),
      nondet n = oneOf(t.todo)
      val rest = { ...t, todo: t.todo.exclude(Set(n)) }
      val r = if (published(s, n)) record(s, x, n, Announcement) else { st: s, sent: false }
      s' = advanceSub(setSub(if (r.sent) deliver(r.st, x, n, Announcement) else r.st, x, rest), x),
    },
  }

  action step = any {
    nondet p = oneOf(PUBLISHERS)
    any {
      announce(p), publishRouted(p), publishRouteFailed(p), publishNamespaceDone(p),
      withdrawalRead(p), disconnect(p), leaveNext(p), pubTell(p),
    },
    nondet x = oneOf(SUBSCRIBERS)
    any {
      subscribeNamespace(x), subscribeRouted(x), subscribeRouteFailed(x), subscribeRefused(x),
      updatePrefix(x), updateRouted(x), updateRouteFailed(x), updateRefused(x),
      updateReleased(x), updateForgot(x),
      unsubscribeNamespace(x), unsubscribeReleased(x), unsubscribeForgot(x),
      offerRecord(x),
    },
  }

  // --- Properties -------------------------------------------------------

  // Every request is answered and every loop has ended.
  val quiescent: bool = and {
    PUBLISHERS.forall(p => s.pubTasks.get(p).phase == PIdle),
    SUBSCRIBERS.forall(x => s.subTasks.get(x).phase == SIdle),
  }

  // With nothing in flight, a subscriber has been told, net, exactly the
  // published namespaces under its prefix, and nothing once it has no
  // subscription. The publisher's own session is told of its namespace too
  // (`notify_local_subscribers`).
  val quiescentToldMatchesDirectory: bool =
    quiescent implies SUBSCRIBERS.forall(x =>
      val m = s.moqt.get(x)
      toldTo(s, x) == (if (m.state == MActive) publishedUnder(s, m.prefix) else Set()))

  // With nothing in flight, the relay's record of a session is what the
  // session was told: a namespace recorded but not told is never announced,
  // one told but not recorded never withdrawn.
  val quiescentRecordMatchesTold: bool =
    quiescent implies SUBSCRIBERS.forall(x => s.rec.get(x) == toldTo(s, x))

  // The relay never announces a namespace the subscriber was told of and not
  // told the end of (moqt swallows it on a subscription on draft-18).
  val noDuplicateAnnouncement: bool = not(s.duplicate)

  // The relay never withdraws a namespace from a subscription covering it
  // that was not told of it. Once the subscription ended or moved, moqt
  // drops the DONE, and the subscriber counts the namespaces as ended.
  val noDoneForUntold: bool = not(s.untoldDone)

  // The relay announces a namespace only to a subscription covering it;
  // otherwise moqt sends it as a PUBLISH_NAMESPACE request outside any.
  val noStrayAnnouncement: bool = not(s.strayAnnounced)

  val safety: bool = and {
    quiescentToldMatchesDirectory,
    quiescentRecordMatchesTold,
    noDuplicateAnnouncement,
    noDoneForUntold,
    noStrayAnnouncement,
  }
}

module relay_namespace_announcements {
  import namespace_announcements(
    PUBLISHERS = Set("p1", "p2"),
    SUBSCRIBERS = Set("x1", "x2"),
    NAMESPACES = Set("a/b", "a/c"),
    PREFIXES = Set("a", "a/b"),
    UNDER = Set(("a", "a/b"), ("a", "a/c"), ("a/b", "a/b")),
  ).*
}

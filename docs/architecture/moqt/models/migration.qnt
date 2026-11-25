// A model of a client session that follows GOAWAY: the `Router` task that
// reads every connection's events, migrates, reconnects and retires
// connections, together with what it waits for before closing the previous
// connection.
//
// Code mapping (paths under crates/moqt/src/modules/moqt/):
// - `Router::route` (domains/session/migration_task.rs:202-324): locals
//   `previous` and `migration: Migration` (:141-191) are `router.previous`
//   and `router.migration` (`Migration::{None, Following, Reconnecting}`);
//   `router.currentRead` is `CurrentRead` inside `Migration::Following`
//   (`Migration::None` reads the current connection, `Reconnecting` has seen
//   it closed).
// - `Router::reconnects` (migration_task.rs:326-341), `Router::retire`
//   (:364-385), `Router::finish` (:388-393).
// - `migrate` (migration_task.rs:487-633): `session.move_to` (:597) and the
//   `released` future (:606-630), which ends in `SessionContext::unserved`
//   (domains/session_context.rs:368-375); `carry_subscription` (:680-788)
//   and `Carried::Stays` (:664-678).
// - `SessionContext::{move_to, forget_previous}` (session_context.rs:395, 461):
//   the session's `connections` mirror `router.current`/`router.previous`,
//   since `move_to` runs in the same poll that hands `Migrated` to the router.
// - `Session::close` (domains/session.rs:137-150) closes only
//   `SessionContext::connection()`, the current one; `Connection::close`
//   sets `closed_locally` even on a connection the peer already closed
//   (domains/connection.rs:608-616).
// - `DisconnectWatchTask` (runtime/tasks/disconnect_watch_task.rs) queues
//   one `Disconnected` per closed connection. `Connection` holds the event
//   sender (connection.rs:74), so a connection's events never end with
//   `None` while the router keeps it.
// - The served SUBSCRIBE: `SubscribeHandler::new` calls `serve()` when the
//   control receive task reads the SUBSCRIBE; a refusal or a handler dropped
//   unanswered undoes it (control_plane/handler/subscribe_handler.rs);
//   `Publisher::publish_done` writes PUBLISH_DONE, then calls `stop_serving`
//   + `note_served_changed` (domains/publisher.rs; `remove_publication`
//   counts a publication as served until then), and
//   `notify_end` swallows `ConnectionLost` (version.rs:1055-1063);
//   `Router::unsubscribed` calls `stop_serving` (migration_task.rs:296-303).
//
// One action = one step of one task between awaits: the router handling one
// `select!` arm (the `Ok` arm of a migration includes `move_to`, which runs
// in the same poll), the peer sending a message or closing, the application
// calling one half of an API around its `.await`.
// The `released` future is split into its timers (`graceElapsed`: cutover,
// grace and CUTOVER_LIMIT, which always end) and `unserved`.
//
// Actors: the peer of each connection (GOAWAY, SUBSCRIBE, UNSUBSCRIBE,
// PUBLISH_DONE, close), the application (Session::close, SUBSCRIBE_OK,
// publish_done, UNSUBSCRIBE), the router. Connections 0..2: one GOAWAY
// migration or reconnect per connection.
//
// Instances: `migration_current` (today's code) and `migration_fixed` (all
// FIX_* switches on). Check (from docs/architecture/moqt/models):
//   quint typecheck migration.qnt
//   quint run --main migration_current --invariant safety --max-samples 5000 --max-steps 30 migration.qnt
//   quint verify --backend tlc --main migration_current --invariant safety migration.qnt
//   quint verify --backend tlc --main migration_fixed --invariant safety migration.qnt
// (`--invariant <name>` for one property).
module migration {
  // Connection ids; the session starts on 0 and each migration opens the next.
  const CONNECTIONS: Set[int]
  // The `Ok` arm of a migration ends the session (closing the new
  // connection) when the connection it moves from was closed locally, and
  // retires that connection at once when the router already read its
  // Disconnected (as after a reconnect).
  const FIX_CLOSED_DURING_MIGRATION: bool
  // `finish` retires the previous connection too.
  const FIX_FINISH_CLOSES_PREVIOUS: bool
  // `retire` reports a subscription that stays on the retired connection
  // (`Carried::Stays`) as ended to the application, as it reports served
  // SUBSCRIBEs, so the application can subscribe again.
  const FIX_REPORT_STAYING_END: bool
  // `publish_done` writes PUBLISH_DONE before `stop_serving`.
  const FIX_DONE_BEFORE_STOP: bool
  // The SUBSCRIBE enters `served` when it arrives (in the control receive
  // task, ordered before its UNSUBSCRIBE), not after SUBSCRIBE_OK.
  const FIX_SERVE_ON_RECEIPT: bool

  // A connection's transport.
  type Transport = Unopened | Open | Closed

  type Migration =
    | NoMigration
    | Following(int)     // GOAWAY: `migrate` to the named connection
    | Reconnecting(int)  // `reconnect` after the current connection was lost

  type PreviousSlot = NoPrevious | Previous(int)

  // Whether the router still reads the current connection (`current_closed`).
  type CurrentRead = Reading | ClosedSeen

  type RouterPhase = Routing | Finished

  // The SUBSCRIBE the peer of connection 0 sent this side, as this side's
  // application and `served` see it.
  type Served =
    | SrvNone
    | SrvOffered      // SessionEvent::Subscribe handed to the application
    | SrvOkSent       // SUBSCRIBE_OK written, `serve()` not called yet
    | SrvServing      // `ok` returned
    | SrvDoneUnsent   // publish_done: `stop_serving` done, PUBLISH_DONE not written
    | SrvDoneWritten  // publish_done (fixed order): PUBLISH_DONE written, `stop_serving` not done
    | SrvDone         // publish_done returned
    | SrvRetired      // `retire` reported it as unsubscribed

  type PeerUnsubscribe = NotSent | Queued | Handled

  // The SUBSCRIBE this side sent on connection 0 and carries across
  // migrations.
  type Subscription =
    | SubActive(int)   // read on this connection (`Carried::Moved` moves it)
    | SubStaying(int)  // `Carried::Stays`: the new server refused it
    | SubEnded         // PUBLISH_DONE received or UNSUBSCRIBE sent
    | SubCut           // its connection closed under it with no end reported

  type State = {
    transports: int -> Transport,
    // `Connection::closed_locally`
    closedLocally: Set[int],
    // Unread events per connection.
    goAwayQueued: Set[int],
    goAwaySent: Set[int],
    disconnectQueued: Set[int],
    unsubscribe: PeerUnsubscribe,
    // Router locals (and `SessionContext::connections`).
    current: int,
    currentRead: CurrentRead,
    previous: PreviousSlot,
    migration: Migration,
    phase: RouterPhase,
    // The timers of `released` ran out for the current previous connection.
    graceDone: bool,
    // `Session::close` was called.
    sessionClosed: bool,
    // `served` of connection 0 holds the SUBSCRIBE.
    served: bool,
    srv: Served,
    sub: Subscription,
    // Observations of violations at the moment they happen.
    publishDoneDropped: bool,
    stayingCut: bool,
  }

  var s: State

  def isOpen(c: int): bool = s.transports.get(c) == Open

  // Connections whose events the router reads: the current one until its
  // Disconnected, and the previous one.
  def readsCurrent: bool = s.phase == Routing and s.currentRead == Reading
  def readsPrevious(c: int): bool = s.phase == Routing and s.previous == Previous(c)
  def reads(c: int): bool = (c == s.current and readsCurrent) or readsPrevious(c)

  // `Connection::close`: sets `closed_locally`, closes the transport, and the
  // disconnect watcher queues Disconnected.
  def closeLocally(st: State, c: int): State = {
    ...st,
    closedLocally: st.closedLocally.union(Set(c)),
    transports: if (st.transports.get(c) == Open) st.transports.set(c, Closed) else st.transports,
    disconnectQueued:
      if (st.transports.get(c) == Open) st.disconnectQueued.union(Set(c)) else st.disconnectQueued,
  }

  // `Router::retire`: close with NO_ERROR, `forget_previous`, report every
  // SUBSCRIBE still served there as unsubscribed. A subscription left on it
  // ends with it, unreported.
  def retire(st: State, c: int): State =
    val wasOpen = st.transports.get(c) == Open
    val closed = closeLocally(st, c)
    {
      ...closed,
      previous: if (st.previous == Previous(c)) NoPrevious else st.previous,
      served: if (c == 0) false else st.served,
      srv: if (c == 0 and st.served) SrvRetired else st.srv,
      sub:
        if (FIX_REPORT_STAYING_END and st.sub == SubStaying(c)) SubEnded
        else if (st.sub == SubStaying(c) or st.sub == SubActive(c)) SubCut
        else st.sub,
      publishDoneDropped: st.publishDoneDropped or (c == 0 and wasOpen and st.srv == SrvDoneUnsent),
      stayingCut: st.stayingCut or (not(FIX_REPORT_STAYING_END) and st.sub == SubStaying(c)),
    }

  def retirePrevious(st: State): State = match st.previous {
    | NoPrevious => st
    | Previous(p) => retire(st, p)
  }

  // `Router::finish` (the router returns; nothing reads events any more).
  // The session ends, so whatever closing the previous connection cuts is
  // not held against it.
  def finish(st: State): State =
    val retired = if (FIX_FINISH_CLOSES_PREVIOUS) retirePrevious(st) else st
    {
      ...retired,
      phase: Finished,
      migration: NoMigration,
      publishDoneDropped: st.publishDoneDropped,
      stayingCut: st.stayingCut,
    }

  def nextConnection: int = s.current + 1

  // ---- peers ----

  action peerSendsGoAway: bool = all {
    isOpen(s.current),
    not(s.goAwaySent.contains(s.current)),
    s' = {
      ...s,
      goAwaySent: s.goAwaySent.union(Set(s.current)),
      goAwayQueued: s.goAwayQueued.union(Set(s.current)),
    },
  }

  action peerCloses(c: int): bool = all {
    isOpen(c),
    s' = {
      ...s,
      transports: s.transports.set(c, Closed),
      disconnectQueued: s.disconnectQueued.union(Set(c)),
    },
  }

  action peerSubscribes: bool = all {
    s.srv == SrvNone,
    isOpen(0),
    s' = { ...s, srv: SrvOffered, served: FIX_SERVE_ON_RECEIPT },
  }

  action peerUnsubscribes: bool = all {
    Set(SrvOffered, SrvOkSent, SrvServing).contains(s.srv),
    s.unsubscribe == NotSent,
    isOpen(0),
    s' = { ...s, unsubscribe: Queued },
  }

  // PUBLISH_DONE for this side's subscription.
  action peerEndsSubscription: bool = all {
    match s.sub {
      | SubActive(c) => isOpen(c)
      | SubStaying(c) => isOpen(c)
      | _ => false
    },
    s' = { ...s, sub: SubEnded },
  }

  // ---- application ----

  // `Session::close` closes the current connection.
  action appClosesSession: bool = all {
    not(s.sessionClosed),
    s' = { ...closeLocally(s, s.current), sessionClosed: true },
  }

  // `ok_with_track_alias` up to the SUBSCRIBE_OK write.
  action appSendsSubscribeOk: bool = all {
    s.srv == SrvOffered,
    isOpen(0),
    s' = { ...s, srv: SrvOkSent },
  }

  // `ok_with_track_alias` after the write: `serve()`.
  action appServes: bool = all {
    s.srv == SrvOkSent,
    s' = { ...s, srv: SrvServing, served: s.served or not(FIX_SERVE_ON_RECEIPT) },
  }

  // `publish_done`, first half; `connection_of` finds the connection only
  // while the session holds it.
  action appStartsPublishDone: bool = all {
    s.srv == SrvServing,
    0 == s.current or s.previous == Previous(0),
    if (FIX_DONE_BEFORE_STOP) s' = { ...s, srv: SrvDoneWritten }
    else s' = { ...s, srv: SrvDoneUnsent, served: false },
  }

  // `publish_done`, second half; a lost connection swallows the write.
  action appFinishesPublishDone: bool = any {
    all { s.srv == SrvDoneUnsent, s' = { ...s, srv: SrvDone } },
    all { s.srv == SrvDoneWritten, s' = { ...s, srv: SrvDone, served: false } },
  }

  action appUnsubscribes: bool = all {
    match s.sub {
      | SubActive(_) => true
      | SubStaying(_) => true
      | _ => false
    },
    s' = { ...s, sub: SubEnded },
  }

  // ---- router ----

  // GOAWAY on the current connection: starts `migrate` unless one runs.
  action routerReadsGoAway: bool = all {
    readsCurrent,
    s.goAwayQueued.contains(s.current),
    val read = { ...s, goAwayQueued: s.goAwayQueued.exclude(Set(s.current)) }
    if (s.migration == NoMigration and CONNECTIONS.contains(nextConnection))
      s' = { ...read, migration: Following(nextConnection) }
    else s' = read,
  }

  // UNSUBSCRIBE on connection 0, current or previous: `Router::unsubscribed`.
  action routerReadsUnsubscribe: bool = all {
    reads(0),
    s.unsubscribe == Queued,
    s' = { ...s, unsubscribe: Handled, served: false },
  }

  // Disconnected of the current connection (last in its queue).
  action routerReadsCurrentDisconnected(reconnectable: bool): bool = all {
    readsCurrent,
    s.disconnectQueued.contains(s.current),
    not(s.goAwayQueued.contains(s.current)),
    not(s.current == 0 and s.unsubscribe == Queued),
    val read = {
      ...s,
      disconnectQueued: s.disconnectQueued.exclude(Set(s.current)),
      currentRead: ClosedSeen,
    }
    if (s.migration != NoMigration) s' = read
    // `reconnects`: not after a local close; a peer close reconnects when it
    // carries no termination code (or a refreshable token expiry).
    else if (not(s.closedLocally.contains(s.current)) and reconnectable
             and CONNECTIONS.contains(nextConnection))
      s' = { ...read, migration: Reconnecting(nextConnection) }
    else s' = finish(read),
  }

  // Disconnected of the previous connection.
  action routerReadsPreviousDisconnected: bool = all {
    s.phase == Routing,
    match s.previous {
      | NoPrevious => false
      | Previous(p) => and {
        s.disconnectQueued.contains(p),
        not(s.goAwayQueued.contains(p)),
        not(p == 0 and s.unsubscribe == Queued),
      }
    },
    match s.previous {
      | NoPrevious => s' = s
      | Previous(p) => s' = retire({ ...s, disconnectQueued: s.disconnectQueued.exclude(Set(p)) }, p)
    },
  }

  // The timers inside `released`.
  action graceElapsed: bool = all {
    s.previous != NoPrevious,
    not(s.graceDone),
    s' = { ...s, graceDone: true },
  }

  // `released` resolved: its timers ran out and `unserved` returned.
  action routerReleasesPrevious: bool = all {
    s.phase == Routing,
    s.graceDone,
    match s.previous {
      | NoPrevious => false
      | Previous(p) => and {
        not(p == 0 and s.served),
      }
    },
    s' = retirePrevious(s),
  }

  // `migrate`/`reconnect` returned Ok: the arm runs in the same poll as
  // `move_to`. `stays`: the new server refused the carried subscription.
  action routerMigrated(stays: bool): bool = all {
    s.phase == Routing,
    match s.migration {
      | NoMigration => false
      | Following(n) => s.transports.get(n) == Unopened
      | Reconnecting(n) => s.transports.get(n) == Unopened
    },
    val n = match s.migration {
      | Following(n) => n
      | Reconnecting(n) => n
      | NoMigration => -1
    }
    val movedFrom = s.current
    val lost = match s.migration {
      | Reconnecting(_) => true
      | _ => FIX_CLOSED_DURING_MIGRATION and s.currentRead == ClosedSeen
    }
    val older = retirePrevious(s)
    val moved = {
      ...older,
      transports: older.transports.set(n, Open),
      current: n,
      currentRead: Reading,
      previous: Previous(movedFrom),
      migration: NoMigration,
      graceDone: false,
      sub: if (older.sub == SubActive(movedFrom)) {
        if (stays) SubStaying(movedFrom) else SubActive(n)
      } else older.sub,
    }
    if (FIX_CLOSED_DURING_MIGRATION and s.closedLocally.contains(movedFrom))
      // The session was closed while it migrated: it ends.
      s' = finish(closeLocally(retire(moved, movedFrom), n))
    else if (lost) s' = retire(moved, movedFrom)
    else s' = moved,
  }

  action routerMigrationFailed: bool = all {
    s.phase == Routing,
    s.migration != NoMigration,
    val failed = { ...s, migration: NoMigration }
    if (s.currentRead == ClosedSeen) s' = finish(failed) else s' = failed,
  }

  action init = s' = {
    transports: CONNECTIONS.mapBy(c => if (c == 0) Open else Unopened),
    closedLocally: Set(),
    goAwayQueued: Set(),
    goAwaySent: Set(),
    disconnectQueued: Set(),
    unsubscribe: NotSent,
    current: 0,
    currentRead: Reading,
    previous: NoPrevious,
    migration: NoMigration,
    phase: Routing,
    graceDone: false,
    sessionClosed: false,
    served: false,
    srv: SrvNone,
    sub: SubActive(0),
    publishDoneDropped: false,
    stayingCut: false,
  }

  action step = any {
    peerSendsGoAway,
    nondet c = CONNECTIONS.oneOf()
    peerCloses(c),
    peerSubscribes,
    peerUnsubscribes,
    peerEndsSubscription,
    appClosesSession,
    appSendsSubscribeOk,
    appServes,
    appStartsPublishDone,
    appFinishesPublishDone,
    appUnsubscribes,
    routerReadsGoAway,
    routerReadsUnsubscribe,
    nondet reconnectable = Set(true, false).oneOf()
    routerReadsCurrentDisconnected(reconnectable),
    routerReadsPreviousDisconnected,
    graceElapsed,
    routerReleasesPrevious,
    nondet stays = Set(true, false).oneOf()
    routerMigrated(stays),
    routerMigrationFailed,
  }

  // B1: a closed previous connection still has its Disconnected queued, so
  // the router will retire it (and report its SUBSCRIBEs as unsubscribed);
  // otherwise only `released` can, and `unserved` waits for an UNSUBSCRIBE
  // that a closed connection never delivers.
  val closedPreviousIsNoticed: bool = match s.previous {
    | NoPrevious => true
    | Previous(p) =>
      (s.phase == Routing and s.transports.get(p) == Closed) implies s.disconnectQueued.contains(p)
  }

  // B1: after `Session::close` the session never runs on an open connection.
  val closedSessionStaysClosed: bool =
    s.sessionClosed implies not(isOpen(s.current))

  // Once the session ended (`Disconnected` delivered), no connection of it
  // is left open (`Session::close` waits for `Endpoint::wait_idle`).
  val endedSessionLeavesNoConnectionOpen: bool =
    s.phase == Finished implies CONNECTIONS.forall(c => not(isOpen(c)))

  // B3: retiring the previous connection never silently cuts a subscription
  // that the new server refused and that therefore stays on it: the
  // application learns that it ended.
  val stayingSubscriptionIsNotCut: bool = not(s.stayingCut)

  // A publish_done the application started on an open connection reaches the
  // peer before this side closes that connection.
  val publishDoneIsWrittenBeforeClose: bool = not(s.publishDoneDropped)

  // `served` never holds a SUBSCRIBE whose UNSUBSCRIBE was handled (it would
  // keep `unserved` from resolving until the peer closes the connection).
  val unsubscribedIsNotServed: bool =
    not(s.served and s.unsubscribe == Handled)

  val safety: bool = and {
    closedPreviousIsNoticed,
    closedSessionStaysClosed,
    endedSessionLeavesNoConnectionOpen,
    stayingSubscriptionIsNotCut,
    publishDoneIsWrittenBeforeClose,
    unsubscribedIsNotServed,
  }
}

// expect: ok
module migration_current {
  import migration(
    CONNECTIONS = 0.to(2),
    FIX_CLOSED_DURING_MIGRATION = true,
    FIX_FINISH_CLOSES_PREVIOUS = true,
    FIX_REPORT_STAYING_END = true,
    FIX_DONE_BEFORE_STOP = true,
    FIX_SERVE_ON_RECEIPT = true,
  ).*
}

// expect: ok
module migration_fixed {
  import migration(
    CONNECTIONS = 0.to(2),
    FIX_CLOSED_DURING_MIGRATION = true,
    FIX_FINISH_CLOSES_PREVIOUS = true,
    FIX_REPORT_STAYING_END = true,
    FIX_DONE_BEFORE_STOP = true,
    FIX_SERVE_ON_RECEIPT = true,
  ).*
}

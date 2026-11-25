// A model of one namespace this side announces with PUBLISH_NAMESPACE, on a
// session that follows GOAWAY (crates/moqt/src/modules/moqt):
// - `Publisher::announce` / `begin_publish_namespace` (domains/publisher.rs):
//   sends PUBLISH_NAMESPACE under the `carried` read lock, then
//   `PublishedNamespaceRecord::insert` (domains/session_context.rs) inserts the
//   record into `SessionContext::published_namespaces` and
//   `CarriedRequests::published_namespaces`, and the handle's
//   `NamespaceRecord` becomes `Recorded`.
// - `PendingPublishNamespace::accepted` (domains/pending_publish_namespace.rs):
//   moves the handle to `NamespaceRecord::Answered`; on Rejected / Timeout /
//   ProtocolViolation, `PublishedNamespaceRecord::forget` removes the `published_namespaces` entry, then
//   awaits the `carried` read lock and removes the carried entry. A lost
//   connection keeps the record. Dropping the handle while `Recorded` drops
//   the `PublishedNamespaceRecord`, whose `Drop` removes the
//   `published_namespaces` entry and spawns the carried removal.
// - The draft-14 late-response path (version/draft14.rs `handle_late_response`):
//   a PUBLISH_NAMESPACE_OK for a dropped `PendingPublishNamespace` is answered
//   with PUBLISH_NAMESPACE_DONE.
// - `Publisher::publish_namespace_done` (domains/publisher.rs):
//   `SessionContext::withdraw_published_namespace` removes every local record
//   of the namespace, awaits the `carried` read lock and removes the carried
//   ones; then it sends PUBLISH_NAMESPACE_DONE on every live connection.
// - An incoming PUBLISH_NAMESPACE_CANCEL (version/draft14/
//   control_message_receive_task.rs, draft-18 `peer_ended` in
//   version/draft18/request_streams.rs, unless it follows a GOAWAY) raises
//   `SessionEvent::PublishNamespaceCancel` to the application and
//   `SessionContext::forget_cancelled_namespace` removes every
//   `published_namespaces` entry of the namespace, then spawns the carried
//   removal, which awaits the `carried` read lock.
// - `migrate` (domains/session/migration_task.rs): under the `carried` write
//   lock, sends PUBLISH_NAMESPACE again on the new connection for every
//   carried namespace and awaits its answer; then the new connection becomes
//   current and the old one stays live until it is retired.
//
// One action is one critical section: one `std::sync::Mutex` hold, or one
// stretch of a task between two `.await`s. The peer of each connection
// answers a PUBLISH_NAMESPACE when it receives it, and a control stream is
// FIFO, so a message's effect at the peer is applied when it is sent.
//
// Switches (each `false` in `publish_namespace_current`):
// - FORGET_ON_DROP: dropping a `PendingPublishNamespace` unanswered forgets
//   its record, as a rejection does.
// - FORGET_ON_CANCEL: an incoming PUBLISH_NAMESPACE_CANCEL forgets the
//   records of the namespace.
// - WITHDRAW_ELSEWHERE: `forget`, once it holds the `carried` lock, sends
//   PUBLISH_NAMESPACE_DONE on the other live connections, where a migration
//   may have announced the namespace again meanwhile.
// - DONE_DURING_BEGIN: the application may call `publish_namespace_done`
//   before `begin_publish_namespace` returned.
//
// Check (from this directory, Java 21+ on PATH; `quint` is
// @informalsystems/quint 0.34):
//   quint typecheck publish_namespace.qnt
//   quint verify --backend tlc --main publish_namespace_current --invariant safety publish_namespace.qnt
//   quint verify --backend tlc --main publish_namespace_fixed --invariant safety publish_namespace.qnt
// and likewise for the other instances at the end of the file.
module publish_namespace {
  const FORGET_ON_DROP: bool
  const FORGET_ON_CANCEL: bool
  const WITHDRAW_ELSEWHERE: bool
  const DONE_DURING_BEGIN: bool

  pure val C1 = "c1"
  pure val C2 = "c2"
  pure val CONNS = Set(C1, C2)

  // The application's `PendingPublishNamespace` and what `accepted` told it.
  type Handle =
    | NoHandle // not announced yet
    | Sending  // PUBLISH_NAMESPACE written, `carried` read lock held, no record yet
    | Pending  // `begin_publish_namespace` returned; the answer is not taken
    | Accepted // `accepted` returned Ok
    | Lost     // `accepted` returned SessionClosed; the record stays for the migration
    | Refused  // `accepted` returned Rejected
    | Dropped  // dropped without `accepted`

  // What else the application did or heard about the namespace.
  type Intent =
    | Wanted         // nothing but the handle
    | CancelReceived // `SessionEvent::PublishNamespaceCancel`
    | Withdrawing    // `publish_namespace_done` removed the local records
    | Withdrawn      // `publish_namespace_done` returned

  // The answer to the application's PUBLISH_NAMESPACE, sent and not yet taken.
  type Answer = NoAnswer | OkInFlight | ErrorInFlight | ClosedInFlight

  // `PublishedNamespaceRecord::forget` between its two steps, with the
  // connection whose answer or cancel started it.
  type Forget = NoForget | AwaitCarried(str)

  type Migration = NotMoved | Locked | Moved

  type State = {
    handle: Handle,
    intent: Intent,
    // The connection the application's PUBLISH_NAMESPACE went on.
    conn: str,
    answer: Answer,
    forget: Forget,
    // `SessionContext::published_namespaces` holds the record:
    // `Session::publishes_namespace` answers this.
    published: bool,
    // `CarriedRequests::published_namespaces` holds the record: a migration
    // announces it again.
    carried: bool,
    migration: Migration,
    current: str,
    live: Set[str],
    // Whether the peer of each connection considers the namespace announced.
    peer: str -> bool,
  }

  var s: State

  action init = s' = {
    handle: NoHandle,
    intent: Wanted,
    conn: C1,
    answer: NoAnswer,
    forget: NoForget,
    published: false,
    carried: false,
    migration: NotMoved,
    current: C1,
    live: Set(C1),
    peer: CONNS.mapBy(_ => false),
  }

  def withdrawnOn(conns: Set[str]): str -> bool =
    CONNS.mapBy(c => if (conns.contains(c)) false else s.peer.get(c))

  // `announce` up to `send_publish_namespace` returning: holds the `carried`
  // read lock, so no migration runs. The peer answers at once.
  action begin(accepts: bool): bool = all {
    s.handle == NoHandle,
    s.migration != Locked,
    s' = {
      ...s,
      handle: Sending,
      conn: s.current,
      answer: if (accepts) OkInFlight else ErrorInFlight,
      peer: if (accepts) s.peer.set(s.current, true) else s.peer,
    },
  }

  // `announce` after the send: inserts both records, drops the read lock.
  action record = all {
    s.handle == Sending,
    s' = { ...s, handle: Pending, published: true, carried: true },
  }

  // `forget`'s first step: the `published_namespaces` entry goes.
  def forgetting(from: str): State =
    { ...s, published: false, forget: AwaitCarried(from) }

  // `accepted` takes the answer; or, for a dropped handle, the draft-14
  // late-response path does.
  action takeAnswer = all {
    s.answer != NoAnswer,
    Set(Pending, Dropped).contains(s.handle),
    val taken = { ...s, answer: NoAnswer }
    if (s.handle == Dropped)
      // A late PUBLISH_NAMESPACE_OK is withdrawn; anything else is dropped.
      s' = if (s.answer == OkInFlight) { ...taken, peer: withdrawnOn(Set(s.conn)) } else taken
    else match s.answer {
      | OkInFlight => s' = { ...taken, handle: Accepted }
      | ErrorInFlight =>
        s' = { ...taken, handle: Refused, published: false, forget: AwaitCarried(s.conn) }
      | ClosedInFlight => s' = { ...taken, handle: Lost }
      | NoAnswer => s' = s
    },
  }

  // The application drops its `PendingPublishNamespace`.
  action dropHandle = all {
    s.handle == Pending,
    if (FORGET_ON_DROP and s.forget == NoForget)
      s' = { ...forgetting(s.conn), handle: Dropped }
    else
      s' = { ...s, handle: Dropped },
  }

  // The connection `forget` started from, or "" when it is not running.
  def forgetFrom: str = match s.forget {
    | NoForget => ""
    | AwaitCarried(c) => c
  }

  // `forget`'s second step, under the `carried` read lock.
  action forgetCarried = all {
    s.forget != NoForget,
    s.migration != Locked,
    s' = {
      ...s,
      carried: false,
      forget: NoForget,
      peer: if (WITHDRAW_ELSEWHERE) withdrawnOn(s.live.exclude(Set(forgetFrom))) else s.peer,
    },
  }

  // `publish_namespace_done` up to `read_carried().await`.
  action done = all {
    s.handle != NoHandle,
    DONE_DURING_BEGIN or s.handle != Sending,
    s.intent == Wanted,
    s' = { ...s, intent: Withdrawing, published: false },
  }

  // `publish_namespace_done` after the read lock: removes the carried
  // records and sends PUBLISH_NAMESPACE_DONE on every live connection.
  action doneFinish = all {
    s.intent == Withdrawing,
    s.migration != Locked,
    s' = { ...s, intent: Withdrawn, carried: false, peer: withdrawnOn(s.live) },
  }

  // The peer of `c` sends PUBLISH_NAMESPACE_CANCEL; on the connection of the
  // application's request it follows any answer still in flight.
  action peerCancels(c: str): bool = all {
    s.live.contains(c),
    s.peer.get(c),
    c != s.conn or s.answer == NoAnswer,
    val heard = {
      ...s,
      peer: s.peer.set(c, false),
      intent: if (s.intent == Wanted) CancelReceived else s.intent,
    }
    s' = if (FORGET_ON_CANCEL and s.forget == NoForget)
      { ...heard, published: false, forget: AwaitCarried(c) }
    else heard,
  }

  // `migrate` takes the `carried` write lock once no request holds the read lock.
  action migrateLock = all {
    s.migration == NotMoved,
    s.handle != Sending,
    s' = { ...s, migration: Locked },
  }

  // `migrate` announces every carried namespace on the new connection, which
  // accepts, makes it current and drops the write lock.
  action migrateFinish = all {
    s.migration == Locked,
    s' = {
      ...s,
      migration: Moved,
      current: C2,
      live: Set(C1, C2),
      peer: s.peer.set(C2, s.carried),
    },
  }

  // The old connection closes; an answer still on it becomes SessionClosed.
  action retireOld = all {
    s.migration == Moved,
    s.live.contains(C1),
    s' = {
      ...s,
      live: Set(C2),
      peer: s.peer.set(C1, false),
      answer: if (s.conn == C1 and s.answer != NoAnswer) ClosedInFlight else s.answer,
    },
  }

  action step = any {
    begin(true),
    begin(false),
    record,
    takeAnswer,
    dropHandle,
    forgetCarried,
    done,
    doneFinish,
    peerCancels(C1),
    peerCancels(C2),
    migrateLock,
    migrateFinish,
    retireOld,
  }

  // The application still holds the announcement: it has a handle that was
  // not refused or dropped, heard no cancel and did not withdraw it.
  val wanted: bool =
    Set(Pending, Accepted, Lost).contains(s.handle) and s.intent == Wanted

  // No message, answer, lock or two-step call is in flight.
  val settled: bool = and {
    s.answer == NoAnswer,
    s.forget == NoForget,
    s.handle != Sending,
    s.intent != Withdrawing,
    s.migration != Locked,
  }

  // `Session::publishes_namespace` is true exactly while the application
  // holds the announcement (its doc: "has neither withdrawn it nor had it
  // refused").
  val publishesWhatTheAppHolds: bool = settled implies (s.published == wanted)

  // A migration announces the namespace again exactly while the application
  // holds the announcement.
  val carriesWhatTheAppHolds: bool = settled implies (s.carried == wanted)

  // The peer of the current connection considers the namespace announced
  // exactly while the application holds the announcement.
  val peerAnnouncesWhatTheAppHolds: bool =
    settled implies (s.peer.get(s.current) == wanted)

  val safety: bool = and {
    publishesWhatTheAppHolds,
    carriesWhatTheAppHolds,
    peerAnnouncesWhatTheAppHolds,
  }
}

// expect: violation
module publish_namespace_current {
  import publish_namespace(
    FORGET_ON_DROP = true,
    FORGET_ON_CANCEL = true,
    WITHDRAW_ELSEWHERE = false,
    DONE_DURING_BEGIN = false,
  ).*
}

// expect: ok
module publish_namespace_fixed {
  import publish_namespace(
    FORGET_ON_DROP = true,
    FORGET_ON_CANCEL = true,
    WITHDRAW_ELSEWHERE = true,
    DONE_DURING_BEGIN = false,
  ).*
}

// Each fix but one: shows the remaining bug on its own.
// expect: violation
module publish_namespace_without_drop_fix {
  import publish_namespace(
    FORGET_ON_DROP = false,
    FORGET_ON_CANCEL = true,
    WITHDRAW_ELSEWHERE = true,
    DONE_DURING_BEGIN = false,
  ).*
}

// expect: violation
module publish_namespace_without_cancel_fix {
  import publish_namespace(
    FORGET_ON_DROP = true,
    FORGET_ON_CANCEL = false,
    WITHDRAW_ELSEWHERE = true,
    DONE_DURING_BEGIN = false,
  ).*
}

// expect: violation
module publish_namespace_without_withdraw_fix {
  import publish_namespace(
    FORGET_ON_DROP = true,
    FORGET_ON_CANCEL = true,
    WITHDRAW_ELSEWHERE = false,
    DONE_DURING_BEGIN = false,
  ).*
}

// With every fix, an application that withdraws the namespace before
// `begin_publish_namespace` returned still leaves a record behind.
// expect: violation
module publish_namespace_done_during_begin {
  import publish_namespace(
    FORGET_ON_DROP = true,
    FORGET_ON_CANCEL = true,
    WITHDRAW_ELSEWHERE = true,
    DONE_DURING_BEGIN = true,
  ).*
}

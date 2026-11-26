// A model of one SUBSCRIBE on the subscriber side of a draft-14 session:
// from SUBSCRIBE_OK to the end of its sink after UNSUBSCRIBE or PUBLISH_DONE,
// across the tasks that touch it. Paths are relative to
// crates/moqt/src/modules/.
//
// Each action is one atomic step of one actor:
// - `peerOk`, `peerSendsStream`, `peerDone`: the publisher writes
//   SUBSCRIBE_OK, opens a subgroup stream, writes PUBLISH_DONE with the
//   Stream Count of the streams it opened. Control messages arrive in order;
//   data streams arrive independently of them (`streamArrives`, which is
//   `ObjectSinks::notify` under `Connection::object_sinks`).
// - `controlTask`: one message handled by the control receive task
//   (moqt/version/draft14/control_message_receive_task.rs): SUBSCRIBE_OK is
//   handed to the caller through a oneshot and the task reads the next
//   message once the caller accepted or dropped it; PUBLISH_DONE is pushed
//   to `PublishDoneExpiryTask` and to the session's events. draft-18's
//   request stream task (version/draft18/request_streams.rs) orders a
//   PUBLISH_DONE behind its SUBSCRIBE_OK the same way.
// - The caller of `Subscriber::subscribe_joining`
//   (moqt/domains/subscriber.rs:187-275): `callerGotOk` takes the response
//   from the oneshot; `callerRegisters` is
//   `SessionContext::register_data_receiver` (moqt/domains/session_context.rs:486-519:
//   installs the sink and inserts `receiver_map` under the sinks lock);
//   `callerReturns` inserts the carried record (subscriber.rs:253) and calls
//   `response.accept()`; `clientWraps` is `client::Subscription::new`
//   (client/subscription.rs), where the `done` waiter was registered
//   without DONE_BEFORE_SUBSCRIBE.
// - `PublishDoneExpiryTask` (moqt/runtime/tasks/publish_done_expiry_task.rs:41-82):
//   `expiryLooksUp` is `Countdown::look_up` in the select branch (`ELooked`
//   with `watch` = `Countdown::Counting`),
//   `expiryForgets` the first poll of the pushed future
//   (`forget_ended_subscription`), `expiryFires` the timeout or the
//   `wait_for` ending and `SessionContext::end_subscription`. Time is not
//   modeled: the 10 s grace may end at any point, so `expiryFires` is
//   enabled whenever the wait could end; `graceOnlyWhenNeeded` states when it must not be the only way out.
// - The session's events (`eventLoopRoutes`, `SessionEventSender::send` in
//   moqt/domains/session_events.rs): a PUBLISH_DONE event resolves the
//   `SubscriptionEnd` registered for the request, or is dropped.
//   `sessionEnds` is the session's Disconnected there.
// - The application: `appClaims` is `Subscriber::accept_data_receiver`
//   (subscriber.rs:1069, called lazily by `TrackReader`, after the Joining
//   FETCH for `subscribe_current_group`); `appUpdates` is
//   `update_forward`/`end_before_group`/`request_new_group`; `appUnsubscribes`
//   is `Subscriber::unsubscribe` (subscriber.rs:837-851).
//
// Switches (`subscription_current` turns on the ones the code implements):
// - ACCEPT_BEFORE_NEXT: the control receive task reads the next message only
//   once the caller accepted (or dropped) the SUBSCRIBE_OK it handed over.
// - DONE_BEFORE_SUBSCRIBE: the `done` waiter (`SubscriptionEnds::register`)
//   is registered before the SUBSCRIBE_OK is accepted, so, with
//   ACCEPT_BEFORE_NEXT, before its PUBLISH_DONE is read; registered after
//   the session ended, it has already ended.
// - KEEP_UNCLAIMED_RECEIVER: the expiry's `end_subscription` leaves an
//   unclaimed `receiver_map` entry; dropping the sink's sender ends it once
//   the application has read what it holds.
// - END_TRACKED: the subscriber records a request as ended on PUBLISH_DONE or
//   UNSUBSCRIBE (`Connection::end_subscription`) and sends nothing more for
//   it.
//
// Check (from docs/architecture/moqt/models, Java 21+):
//   quint typecheck subscription.qnt
//   quint verify --backend tlc --main subscription_current --invariant safety subscription.qnt   # passes
//   (likewise for the other instances at the end of the file)
module subscription {
  const MAX_STREAMS: int
  const ACCEPT_BEFORE_NEXT: bool
  const DONE_BEFORE_SUBSCRIBE: bool
  const KEEP_UNCLAIMED_RECEIVER: bool
  const END_TRACKED: bool

  type Peer =
    | PAnswering // SUBSCRIBE received, not answered
    | PServing   // SUBSCRIBE_OK sent
    | PDone      // PUBLISH_DONE sent

  type Control = SubscribeOk | PublishDone(int)

  type Caller =
    | CWaiting    // awaiting the SUBSCRIBE_OK oneshot
    | CGotOk      // holds SUBSCRIBE_OK, sink not installed
    | CRegistered // `register_data_receiver` done
    | CReturned   // carried record inserted, response accepted, `subscribe` returned
    | CWrapped    // `client::Subscription::new` ran

  // The `ObjectSink` of the subscription's track alias.
  type Sink =
    | SAbsent
    | SBuffer(int)   // streams buffered as an unknown alias
    | SReceiver(int) // installed; the value is `arrived_streams`
    | SCancelled

  // The `receiver_map` entry of the subscription.
  type Channel =
    | NoChannel
    | Unclaimed(int) // registered; streams queued in it
    | Claimed        // taken by `accept_data_receiver`
    | Removed        // removed by `cancel_subscription`
    | Ended(int)     // the sink's sender is gone; streams still queued

  type Reader = RIdle | RReading | RNoReceiver

  type Expiry =
    | EIdle
    | EQueued(int) // PUBLISH_DONE with its Stream Count in the channel
    | ELooked({ count: int, watch: bool })
    | EWaiting({ count: int, watch: bool })
    | ECancelled

  // The `done` waiter of the subscription (`SubscriptionEnd`).
  type Done = DUnregistered | DWaiting | DResolved | DClosed

  type State = {
    peer: Peer,
    peerStreams: int,
    inFlight: int,
    control: List[Control],
    // The control receive task waits for the caller's accept.
    taskBlocked: bool,
    okInChannel: bool,
    caller: Caller,
    sink: Sink,
    channel: Channel,
    reader: Reader,
    expiry: Expiry,
    carriedRecord: bool,
    forgotten: bool,
    pdEvent: bool,
    pdRouted: bool,
    done: Done,
    routesEnded: bool,
    // The control task handled PUBLISH_DONE, or UNSUBSCRIBE was sent.
    ended: bool,
    // A control message for the request was sent after `ended`.
    msgAfterEnd: bool,
    // Streams that reached the sink were dropped unread with `receiver_map`.
    lost: bool,
  }

  var s: State

  action init = s' = {
    peer: PAnswering,
    peerStreams: 0,
    inFlight: 0,
    control: [],
    taskBlocked: false,
    okInChannel: false,
    caller: CWaiting,
    sink: SAbsent,
    channel: NoChannel,
    reader: RIdle,
    expiry: EIdle,
    carriedRecord: false,
    forgotten: false,
    pdEvent: false,
    pdRouted: false,
    done: if (DONE_BEFORE_SUBSCRIBE) DWaiting else DUnregistered,
    routesEnded: false,
    ended: false,
    msgAfterEnd: false,
    lost: false,
  }

  action peerOk = all {
    s.peer == PAnswering,
    s' = { ...s, peer: PServing, control: s.control.append(SubscribeOk) },
  }

  action peerSendsStream = all {
    s.peer == PServing,
    s.peerStreams < MAX_STREAMS,
    s' = { ...s, peerStreams: s.peerStreams + 1, inFlight: s.inFlight + 1 },
  }

  action peerDone = all {
    s.peer == PServing,
    s' = { ...s, peer: PDone, control: s.control.append(PublishDone(s.peerStreams)) },
  }

  // `ObjectSinks::notify` for a subgroup stream header (`deliver` counts it).
  action streamArrives = all {
    s.inFlight > 0,
    s' = {
      ...s,
      inFlight: s.inFlight - 1,
      sink: match s.sink {
        | SAbsent => SBuffer(1)
        | SBuffer(n) => SBuffer(n + 1)
        | SReceiver(k) => SReceiver(k + 1)
        | SCancelled => SCancelled
      },
      channel: if (installed) match s.channel {
        | Unclaimed(m) => Unclaimed(m + 1)
        | _ => s.channel
      } else s.channel,
    },
  }

  def headIsOk: bool = s.control.length() > 0 and s.control.head() == SubscribeOk
  def headDone: int =
    if (s.control.length() > 0) match s.control.head() { | PublishDone(n) => n | _ => -1 }
    else -1

  action controlTask = all {
    not(s.taskBlocked),
    if (headIsOk) s' = {
      ...s,
      control: s.control.tail(),
      okInChannel: true,
      taskBlocked: ACCEPT_BEFORE_NEXT,
    } else all {
      headDone >= 0,
      s' = {
        ...s,
        control: s.control.tail(),
        expiry: if (s.expiry == EIdle) EQueued(headDone) else s.expiry,
        pdEvent: true,
        ended: true,
      },
    },
  }

  action callerGotOk = all {
    s.caller == CWaiting,
    s.okInChannel,
    s' = { ...s, caller: CGotOk, okInChannel: false },
  }

  // `register_data_receiver`: `install` drains the buffer through `deliver`,
  // which counts each stream in `arrived_streams`.
  def buffered: int = match s.sink { | SBuffer(n) => n | _ => 0 }

  action callerRegisters = all {
    s.caller == CGotOk,
    s' = {
      ...s,
      caller: CRegistered,
      sink: SReceiver(buffered),
      channel: Unclaimed(buffered),
    },
  }

  action callerReturns = all {
    s.caller == CRegistered,
    s' = { ...s, caller: CReturned, carriedRecord: true, taskBlocked: false },
  }

  def returned: bool = s.caller == CReturned or s.caller == CWrapped

  action clientWraps = all {
    s.caller == CReturned,
    s' = {
      ...s,
      caller: CWrapped,
      done: if (DONE_BEFORE_SUBSCRIBE) s.done else DWaiting,
    },
  }

  def queued: int = match s.expiry { | EQueued(n) => n | _ => -1 }
  def isLooked: bool = match s.expiry { | ELooked(_) => true | _ => false }
  def isWaiting: bool = match s.expiry { | EWaiting(_) => true | _ => false }
  def wait: { count: int, watch: bool } = match s.expiry {
    | ELooked(w) => w
    | EWaiting(w) => w
    | _ => { count: -1, watch: false }
  }
  def installed: bool = match s.sink { | SReceiver(_) => true | _ => false }

  action expiryLooksUp = all {
    queued >= 0,
    s' = { ...s, expiry: ELooked({ count: queued, watch: queued > 0 and installed }) },
  }

  // `forget_ended_subscription` on the current connection.
  action expiryForgets = all {
    isLooked,
    s' = { ...s, expiry: EWaiting(wait), carriedRecord: false, forgotten: true },
  }

  // `SessionContext::cancel_subscription` (`end_subscription` from the
  // expiry): a no-op unless a receiver is installed; otherwise the sink is
  // cancelled and `receiver_map` loses the subscription's entry.
  def cancelled(st: State, byExpiry: bool): State = match st.sink {
    | SReceiver(_) => {
      ...st,
      sink: SCancelled,
      channel: match st.channel {
        | Unclaimed(m) =>
          if (byExpiry and KEEP_UNCLAIMED_RECEIVER) Ended(m) else Removed
        | _ => st.channel
      },
      lost: st.lost or (byExpiry and not(KEEP_UNCLAIMED_RECEIVER) and match st.channel {
        | Unclaimed(m) => m > 0
        | _ => false
      }),
    }
    | _ => st
  }

  // The grace timeout can end any wait; a watch also ends once the count is
  // reached or the sink is gone.
  action expiryFires = all {
    isWaiting,
    s' = { ...cancelled(s, true), expiry: ECancelled },
  }

  action eventLoopRoutes = all {
    s.pdEvent,
    s' = {
      ...s,
      pdEvent: false,
      pdRouted: true,
      done: if (s.done == DWaiting) DResolved else s.done,
    },
  }

  action sessionEnds = all {
    not(s.routesEnded),
    s' = {
      ...s,
      routesEnded: true,
      done: if (s.done == DWaiting) DClosed else s.done,
    },
  }

  action appClaims = all {
    returned,
    s.reader == RIdle,
    s' = match s.channel {
      | Unclaimed(_) => { ...s, channel: Claimed, reader: RReading }
      | Ended(_) => { ...s, channel: Claimed, reader: RReading }
      | _ => { ...s, reader: RNoReceiver }
    },
  }

  def sends: bool = not(END_TRACKED and s.ended)

  action appUpdates = all {
    returned,
    sends,
    not(s.msgAfterEnd),
    s' = { ...s, msgAfterEnd: s.ended },
  }

  action appUnsubscribes = all {
    returned,
    sends,
    not(s.msgAfterEnd),
    s' = { ...cancelled(s, false), carriedRecord: false, ended: true, msgAfterEnd: s.ended },
  }

  action step = any {
    peerOk,
    peerSendsStream,
    peerDone,
    streamArrives,
    controlTask,
    callerGotOk,
    callerRegisters,
    callerReturns,
    clientWraps,
    expiryLooksUp,
    expiryForgets,
    expiryFires,
    eventLoopRoutes,
    sessionEnds,
    appClaims,
    appUpdates,
    appUnsubscribes,
  }

  def arrived: int = match s.sink { | SReceiver(k) => k | _ => -1 }

  // Once every stream PUBLISH_DONE counted has reached an installed sink,
  // the subscription does not wait for the 10 s grace period to end.
  val graceOnlyWhenNeeded: bool =
    not((isLooked or isWaiting) and not(wait.watch) and wait.count > 0
      and arrived >= wait.count)

  // A subscription PUBLISH_DONE ended is not kept as carried, so it is not
  // sent again on the next connection after GOAWAY.
  val endedNotCarried: bool = not(s.forgotten and s.carriedRecord)

  // Once the expiry has ended a subscription, its sink is no longer a live
  // receiver: the application's reader ends.
  val receiverEndsAfterExpiry: bool =
    not(s.expiry == ECancelled and arrived >= 0)

  // `done()` resolves once the session's events carried the PUBLISH_DONE
  // or the session ended.
  val doneResolves: bool =
    not(s.done == DWaiting and (s.pdRouted or s.routesEnded))

  // Streams that reached the subscription before PUBLISH_DONE ended it are
  // readable by the application, however late it starts reading.
  val noLostStreams: bool = not(s.lost)

  // No SUBSCRIBE_UPDATE, REQUEST_UPDATE or UNSUBSCRIBE refers to a request
  // after the subscriber learned it ended (PUBLISH_DONE or its own
  // UNSUBSCRIBE).
  val noMessageAfterEnd: bool = not(s.msgAfterEnd)

  val safety: bool = and {
    graceOnlyWhenNeeded,
    endedNotCarried,
    receiverEndsAfterExpiry,
    doneResolves,
    noLostStreams,
    noMessageAfterEnd,
  }
}

// expect: ok
module subscription_current {
  import subscription(
    MAX_STREAMS = 2,
    ACCEPT_BEFORE_NEXT = true,
    DONE_BEFORE_SUBSCRIBE = true,
    KEEP_UNCLAIMED_RECEIVER = true,
    END_TRACKED = true,
  ).*
}


// Each instance below turns on one fix; check it with the invariants that
// fix guards.
// expect: violation
module subscription_accept_before_next {
  // graceOnlyWhenNeeded, endedNotCarried, receiverEndsAfterExpiry
  import subscription(
    MAX_STREAMS = 2,
    ACCEPT_BEFORE_NEXT = true,
    DONE_BEFORE_SUBSCRIBE = false,
    KEEP_UNCLAIMED_RECEIVER = false,
    END_TRACKED = false,
  ).*
}

// expect: violation
module subscription_done_before_subscribe {
  // doneResolves
  import subscription(
    MAX_STREAMS = 2,
    ACCEPT_BEFORE_NEXT = false,
    DONE_BEFORE_SUBSCRIBE = true,
    KEEP_UNCLAIMED_RECEIVER = false,
    END_TRACKED = false,
  ).*
}

// expect: violation
module subscription_keep_unclaimed {
  // noLostStreams
  import subscription(
    MAX_STREAMS = 2,
    ACCEPT_BEFORE_NEXT = false,
    DONE_BEFORE_SUBSCRIBE = false,
    KEEP_UNCLAIMED_RECEIVER = true,
    END_TRACKED = false,
  ).*
}

// expect: violation
module subscription_end_tracked {
  // noMessageAfterEnd
  import subscription(
    MAX_STREAMS = 2,
    ACCEPT_BEFORE_NEXT = false,
    DONE_BEFORE_SUBSCRIBE = false,
    KEEP_UNCLAIMED_RECEIVER = false,
    END_TRACKED = true,
  ).*
}

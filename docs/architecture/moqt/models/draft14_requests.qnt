// A model of the outgoing requests of one draft-14 session
// (crates/moqt/src/modules/moqt/version/draft14.rs, `ControlChannel`), with
// the control message receive task
// (crates/moqt/src/modules/moqt/version/draft14/control_message_receive_task.rs)
// and a well-behaved peer.
//
// Each action is one atomic step of the implementation:
// - `reserve`: `wait_for_request_id` takes the `request_ids` mutex (and keeps
//   it while blocked, sending one REQUESTS_BLOCKED per limit via
//   `OutgoingRequestIds::block_on`), and, once the next Request ID is below
//   the peer's limit, `send_request` registers the request in
//   `inflight_requests` (`InflightRequests::register`) before writing it.
// - `writeSome` / `writeDone` / `writeFail` / `cancelWrite`: the
//   `queue_with_request_id` write. quinn's `write_all` may write part of the
//   message and then wait for flow control. `BiStreamSender::queue` takes
//   the control stream and advances `OutgoingRequestIds::next`;
//   `QueuedMessage::write` runs in its own task holding the stream, so a
//   cancelled caller neither tears the message nor lets the next request
//   write before it (one `cancelWrite` step stands for that completion).
//   `RegisteredSender` stays `Unsent` until the write is queued, and
//   returns to it when the write fails: its `Drop` then removes the entry;
//   otherwise it turns the entry `Abandoned` (`InflightRequests::abandon`)
//   and closes the session past MAX_ABANDONED_REQUESTS.
// - `dropReceiver` / `dropRegistration`: the caller times out or drops
//   `PendingResponse` (`response()`): the receiver goes first, then
//   `RegisteredSender::drop` locks `inflight_requests`. Another thread may run
//   the receive task in between.
// - `receive`: the receive task takes the entry under the lock
//   (`InflightRequests::take`) and sends it to the caller (a failed send hands
//   back a `ReceivedResponse` whose drop withdraws), or runs
//   `handle_late_response`, or closes the session on an unknown Request ID.
// - `accept` / `dropResponse`: `ReceivedResponse::accept` or its `Drop`
//   (withdrawal of a success response).
// - `streamEnd`: `fail_inflight_requests` (`InflightRequests::close`) when
//   the control stream ends.
// Request IDs step by 1 here (the implementation steps by 2 from 0 or 1).
// MAX_ABANDONED scales MAX_ABANDONED_REQUESTS (100) down.
//
// `draft14_requests_current` is the implementation: UNREGISTER_UNSENT and
// FINISH_CANCELLED_WRITE are on, and it passes, as `draft14_requests_fixed`
// does. Without them it violates `abandonedOnlySent` and `noTornRequest`.
// `draft14_requests_release_before_write` drops the reservation before the
// write (HOLD_RESERVATION = false) and fails `wireIdsInOrder`.
//
// Check (from docs/architecture/moqt/models, Java 21+):
//   quint typecheck draft14_requests.qnt
//   quint verify --backend tlc --main draft14_requests_current --invariant safety draft14_requests.qnt
//   quint verify --backend tlc --main draft14_requests_fixed --invariant safety draft14_requests.qnt
//   quint verify --backend tlc --main draft14_requests_release_before_write --invariant safety draft14_requests.qnt
module draft14_requests {
  const CALLERS: Set[str]
  // Request IDs 0 .. MAX_ID - 1 may be used.
  const MAX_ID: int
  // The peer's SETUP MAX_REQUEST_ID.
  const INITIAL_LIMIT: int
  // Abandoned entries tolerated before the session closes.
  const MAX_ABANDONED: int
  // The reservation (the `request_ids` mutex) is held across the write, as
  // `send_request` does; false advances the ID and releases it first.
  const HOLD_RESERVATION: bool
  // Fix: a request whose write failed or was cancelled is removed from
  // `inflight_requests` instead of being marked `Abandoned`.
  const UNREGISTER_UNSENT: bool
  // Fix: a write that already put bytes on the stream completes even when
  // the caller is dropped (the request then counts as abandoned).
  const FINISH_CANCELLED_WRITE: bool

  // A caller of `reserve_request_id` + `send_request` + `response`.
  type Caller =
    | CIdle
    | CBlocked                   // holds `request_ids`, waits for MAX_REQUEST_ID
    | CWriting(int)              // registered, write pending, no byte written
    | CWritingPartial(int)       // registered, part of the request written
    | CAwaiting(int)             // written, waiting on the oneshot receiver
    | CDropping(int)             // receiver dropped, `RegisteredSender` not yet
    | CHasResponse({ id: int, ok: bool }) // `ReceivedResponse` not yet accepted

  // `Entry`
  type Entry = Waiting(str) | Abandoned

  type State = {
    callers: str -> Caller,
    // `OutgoingRequestIds::next`
    nextId: int,
    // The caller holding `request_ids`, or "".
    holder: str,
    // The peer's Maximum Request ID (exclusive).
    peerLimit: int,
    // `OutgoingRequestIds::blocked` (`RequestsBlockedSent`), -1 for None.
    blockedOn: int,
    // Limits REQUESTS_BLOCKED was sent for, in order.
    blockedSent: List[int],
    // `InflightRequests::Open`
    inflight: int -> Entry,
    // `InflightRequests::Closed`
    streamClosed: bool,
    // The send side failed; every later write fails.
    sendBroken: bool,
    // Request IDs of complete requests on the wire, in order.
    wire: List[int],
    // A partial request was left on the control stream.
    torn: bool,
    // Responses on the wire to us, in order: (id, success).
    toUs: List[{ id: int, ok: bool }],
    // Request IDs the peer answered.
    answered: Set[int],
    // Success responses the receive task took.
    okReceived: Set[int],
    // Success responses a caller accepted.
    accepted: Set[int],
    // Withdrawals (UNSUBSCRIBE, FETCH_CANCEL, ...) sent per Request ID.
    withdrawn: int -> int,
    // The session closed with CONTROL_MESSAGE_TIMEOUT (too many abandoned).
    closedAbandoned: bool,
    // The session closed on a response for an unknown Request ID.
    closedUnknown: bool,
  }

  var s: State

  pure val IDS = 0.to(MAX_ID - 1)
  pure val NOBODY = ""

  def closed: bool = s.closedAbandoned or s.closedUnknown

  pure def idOf(c: Caller): int = match c {
    | CWriting(i) => i
    | CWritingPartial(i) => i
    | CAwaiting(i) => i
    | CDropping(i) => i
    | CHasResponse(r) => r.id
    | _ => -1
  }

  // The response a caller holds, id -1 when none.
  pure def responseOf(c: Caller): { id: int, ok: bool } = match c {
    | CHasResponse(r) => r
    | _ => { id: -1, ok: false }
  }

  pure def ownerOf(e: Entry): str = match e {
    | Waiting(c) => c
    | Abandoned => ""
  }

  pure def isWriting(c: Caller): bool = match c {
    | CWriting(_) => true
    | CWritingPartial(_) => true
    | _ => false
  }

  def abandonedCount(m: int -> Entry): int =
    m.keys().filter(k => m.get(k) == Abandoned).size()

  action init = s' = {
    callers: CALLERS.mapBy(_ => CIdle),
    nextId: 0,
    holder: NOBODY,
    peerLimit: INITIAL_LIMIT,
    blockedOn: -1,
    blockedSent: List(),
    inflight: Map(),
    streamClosed: false,
    sendBroken: false,
    wire: List(),
    torn: false,
    toUs: List(),
    answered: Set(),
    okReceived: Set(),
    accepted: Set(),
    withdrawn: IDS.mapBy(_ => 0),
    closedAbandoned: false,
    closedUnknown: false,
  }

  // `register`: inserts (overwriting any entry of the same ID) unless the
  // control stream is closed.
  def registered(c: str, id: int): int -> Entry =
    if (s.streamClosed) s.inflight else s.inflight.put(id, Waiting(c))

  // The reservation is granted: register, and with HOLD_RESERVATION = false
  // advance the ID and release `request_ids` at once.
  def granted(c: str): State = {
    ...s,
    callers: s.callers.set(c, CWriting(s.nextId)),
    inflight: registered(c, s.nextId),
    nextId: if (HOLD_RESERVATION) s.nextId else s.nextId + 1,
    holder: if (HOLD_RESERVATION) c else NOBODY,
  }

  // `wait_for_request_id` + `register`.
  action reserve(c: str): bool = all {
    not(closed),
    s.callers.get(c) == CIdle,
    s.holder == NOBODY,
    s.nextId < MAX_ID,
    if (s.nextId < s.peerLimit) s' = granted(c)
    else s' = {
      ...s,
      callers: s.callers.set(c, CBlocked),
      holder: c,
      blockedOn: s.peerLimit,
      blockedSent:
        if (s.blockedOn != s.peerLimit) s.blockedSent.append(s.peerLimit)
        else s.blockedSent,
    },
  }

  // `peer_request_id_limit.changed()` woke the blocked waiter.
  action unblock(c: str): bool = all {
    not(closed),
    s.callers.get(c) == CBlocked,
    s.nextId < s.peerLimit,
    s' = granted(c),
  }

  // `reserve_request_id` timed out (CONTROL_MESSAGE_RESPONSE_TIMEOUT).
  action reserveTimeout(c: str): bool = all {
    s.callers.get(c) == CBlocked,
    s' = { ...s, callers: s.callers.set(c, CIdle), holder: NOBODY },
  }

  action writeSome(c: str): bool = all {
    not(closed),
    not(s.sendBroken),
    val i = idOf(s.callers.get(c))
    all {
      s.callers.get(c) == CWriting(i),
      s' = { ...s, callers: s.callers.set(c, CWritingPartial(i)) },
    },
  }

  // The bytes of request `id` are all on the wire: advance and release.
  def written(id: int): State = {
    ...s,
    wire: s.wire.append(id),
    nextId: if (HOLD_RESERVATION) s.nextId + 1 else s.nextId,
    holder: if (HOLD_RESERVATION) NOBODY else s.holder,
  }

  action writeDone(c: str): bool = all {
    not(closed),
    not(s.sendBroken),
    val st = s.callers.get(c)
    all {
      isWriting(st),
      val w = written(idOf(st))
      s' = { ...w, callers: w.callers.set(c, CAwaiting(idOf(st))) },
    },
  }

  // `RegisteredSender::drop`: a still-waiting entry becomes `Abandoned`; the
  // drop that passes MAX_ABANDONED closes the session.
  def abandoned(base: State, id: int): State =
    if (base.inflight.keys().contains(id) and base.inflight.get(id) != Abandoned) {
      val m = base.inflight.set(id, Abandoned)
      { ...base, inflight: m, closedAbandoned: base.closedAbandoned or abandonedCount(m) > MAX_ABANDONED }
    } else base

  // A write that never completed: `?` (or the caller's drop) drops the
  // reservation and `RegisteredSender`.
  def unsent(base: State, c: str, id: int): State = {
    val released = {
      ...base,
      callers: base.callers.set(c, CIdle),
      holder: if (HOLD_RESERVATION) NOBODY else base.holder,
    }
    if (UNREGISTER_UNSENT)
      { ...released, inflight: released.inflight.keys().exclude(Set(id)).mapBy(k => released.inflight.get(k)) }
    else abandoned(released, id)
  }

  action writeFail(c: str): bool = all {
    not(closed),
    val st = s.callers.get(c)
    all {
      isWriting(st),
      s' = unsent({ ...s, sendBroken: true }, c, idOf(st)),
    },
  }

  // The caller's future is dropped during the write.
  action cancelWrite(c: str): bool = all {
    not(closed),
    val i = idOf(s.callers.get(c))
    if (s.callers.get(c) == CWriting(i)) s' = unsent(s, c, i)
    else all {
      s.callers.get(c) == CWritingPartial(i),
      if (FINISH_CANCELLED_WRITE and not(s.sendBroken)) {
        val w = written(i)
        s' = abandoned({ ...w, callers: w.callers.set(c, CIdle) }, i)
      } else s' = unsent({ ...s, torn: true }, c, i),
    },
  }

  // Timeout or drop of `PendingResponse`: the receiver goes first.
  action dropReceiver(c: str): bool = all {
    not(closed),
    val i = idOf(s.callers.get(c))
    all {
      s.callers.get(c) == CAwaiting(i),
      s' = { ...s, callers: s.callers.set(c, CDropping(i)) },
    },
  }

  action dropRegistration(c: str): bool = all {
    not(closed),
    val i = idOf(s.callers.get(c))
    all {
      s.callers.get(c) == CDropping(i),
      s' = abandoned({ ...s, callers: s.callers.set(c, CIdle) }, i),
    },
  }

  // `response()` fails with SessionClosed: the stream end dropped the sender.
  action senderGone(c: str): bool = all {
    val i = idOf(s.callers.get(c))
    all {
      s.callers.get(c) == CAwaiting(i),
      s.streamClosed,
      not(s.inflight.keys().contains(i)),
      s' = { ...s, callers: s.callers.set(c, CIdle) },
    },
  }

  def withdraw(base: State, id: int): State =
    { ...base, withdrawn: base.withdrawn.set(id, base.withdrawn.get(id) + 1) }

  action accept(c: str): bool = all {
    val r = responseOf(s.callers.get(c))
    all {
      r.id >= 0,
      s' = {
        ...s,
        callers: s.callers.set(c, CIdle),
        accepted: if (r.ok) s.accepted.union(Set(r.id)) else s.accepted,
      },
    },
  }

  // `ReceivedResponse::drop` before `accept`.
  action dropResponse(c: str): bool = all {
    not(closed),
    val r = responseOf(s.callers.get(c))
    val b = { ...s, callers: s.callers.set(c, CIdle) }
    all {
      r.id >= 0,
      s' = if (r.ok) withdraw(b, r.id) else b,
    },
  }

  // The peer answers a request it received, once.
  action peerRespond(id: int, ok: bool): bool = all {
    not(closed),
    not(s.torn),
    s.wire.indices().exists(i => s.wire[i] == id),
    not(s.answered.contains(id)),
    s' = { ...s, answered: s.answered.union(Set(id)), toUs: s.toUs.append({ id: id, ok: ok }) },
  }

  // The peer sends MAX_REQUEST_ID (`raise_peer_request_id_limit`).
  action peerRaiseLimit = all {
    not(closed),
    not(s.streamClosed),
    s.peerLimit < MAX_ID,
    s' = { ...s, peerLimit: s.peerLimit + 1 },
  }

  // The receive task handles the next response.
  action receive = all {
    not(closed),
    not(s.streamClosed),
    s.toUs.length() > 0,
    val r = s.toUs.head()
    val b0 = { ...s, toUs: s.toUs.tail(), okReceived: if (r.ok) s.okReceived.union(Set(r.id)) else s.okReceived }
    if (not(s.inflight.keys().contains(r.id))) s' = { ...b0, closedUnknown: true }
    else {
      val b = { ...b0, inflight: s.inflight.keys().exclude(Set(r.id)).mapBy(k => s.inflight.get(k)) }
      val c = ownerOf(s.inflight.get(r.id))
      if (c != NOBODY and s.callers.get(c) == CAwaiting(r.id))
        s' = { ...b, callers: b.callers.set(c, CHasResponse(r)) }
      // `Abandoned`, or the receiver is gone (`send` hands the response
      // back and its drop withdraws).
      else s' = if (r.ok) withdraw(b, r.id) else b
    },
  }

  // `fail_inflight_requests`
  action streamEnd = all {
    not(closed),
    not(s.streamClosed),
    s' = { ...s, streamClosed: true, inflight: Map(), toUs: List(), peerLimit: MAX_ID },
  }

  action step = any {
    nondet c = oneOf(CALLERS)
    any {
      reserve(c),
      unblock(c),
      reserveTimeout(c),
      writeSome(c),
      writeDone(c),
      writeFail(c),
      cancelWrite(c),
      dropReceiver(c),
      dropRegistration(c),
      senderGone(c),
      accept(c),
      dropResponse(c),
    },
    nondet id = oneOf(IDS)
    nondet ok = oneOf(Set(true, false))
    peerRespond(id, ok),
    peerRaiseLimit,
    receive,
    streamEnd,
  }

  // §9.1: this side's requests reach the peer with Request IDs increasing by
  // one step from the first, without gaps.
  val wireIdsInOrder: bool = s.wire.indices().forall(i => s.wire[i] == i)

  // §9.5: no request uses a Request ID at or above the peer's Maximum Request ID.
  val wireUnderLimit: bool = s.wire.length() <= s.peerLimit

  // §9.6: at most one REQUESTS_BLOCKED per Maximum Request ID.
  val oneBlockedPerLimit: bool =
    s.blockedSent.indices().forall(i => s.blockedSent.indices().forall(j =>
      i < j implies s.blockedSent[i] != s.blockedSent[j]))

  // No partial request is left on the control stream (the peer would parse
  // the next message as its continuation).
  val noTornRequest: bool = not(s.torn)

  // The abandoned count, which can close the session, counts only requests
  // the peer received.
  val abandonedOnlySent: bool =
    s.inflight.keys().forall(id => s.inflight.get(id) == Abandoned implies s.wire.indices().exists(i => s.wire[i] == id))

  // A waiting entry belongs to the caller that still waits for that ID, so a
  // response reaches only the caller that sent the request.
  val entryBelongsToItsCaller: bool =
    s.inflight.keys().forall(id => {
      val c = ownerOf(s.inflight.get(id))
      c != NOBODY implies idOf(s.callers.get(c)) == id
    })

  // A well-behaved peer never makes this side close the session for an
  // unknown Request ID.
  val noUnknownResponse: bool = not(s.closedUnknown)

  // Every success response is accepted by its caller, held by it, or
  // withdrawn — exactly one of these.
  val everyOkHandledOnce: bool = IDS.forall(id => {
    val held = CALLERS.exists(c => s.callers.get(c) == CHasResponse({ id: id, ok: true }))
    val n = s.withdrawn.get(id) + (if (s.accepted.contains(id)) 1 else 0) + (if (held) 1 else 0)
    if (s.okReceived.contains(id)) n == 1 else n == 0
  })

  val safety: bool = and {
    wireIdsInOrder,
    wireUnderLimit,
    oneBlockedPerLimit,
    noTornRequest,
    abandonedOnlySent,
    entryBelongsToItsCaller,
    noUnknownResponse,
    everyOkHandledOnce,
  }
}

// expect: ok
module draft14_requests_current {
  import draft14_requests(
    CALLERS = Set("c1", "c2"),
    MAX_ID = 3,
    INITIAL_LIMIT = 1,
    MAX_ABANDONED = 1,
    HOLD_RESERVATION = true,
    UNREGISTER_UNSENT = true,
    FINISH_CANCELLED_WRITE = true,
  ).*
}

// expect: ok
module draft14_requests_fixed {
  import draft14_requests(
    CALLERS = Set("c1", "c2"),
    MAX_ID = 3,
    INITIAL_LIMIT = 1,
    MAX_ABANDONED = 1,
    HOLD_RESERVATION = true,
    UNREGISTER_UNSENT = true,
    FINISH_CANCELLED_WRITE = true,
  ).*
}

// expect: violation
module draft14_requests_release_before_write {
  import draft14_requests(
    CALLERS = Set("c1", "c2"),
    MAX_ID = 3,
    INITIAL_LIMIT = 1,
    MAX_ABANDONED = 1,
    HOLD_RESERVATION = false,
    UNREGISTER_UNSENT = true,
    FINISH_CANCELLED_WRITE = true,
  ).*
}

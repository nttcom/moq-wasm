// A model of draft-18 REQUEST_UPDATE answers on one request stream, both
// directions (docs/spec/draft-ietf-moq-transport-18.txt §10.9: the receiver
// of a REQUEST_UPDATE MUST respond with exactly one REQUEST_OK or
// REQUEST_ERROR, in order on the stream).
//
// Outbound: this side sends REQUEST_UPDATEs and waits for their answers
// (crates/moqt/src/modules/moqt/version/draft18.rs).
// `OQueued` / `OPartial` are a live `QueuedUpdate` and `OWaiting` an
// `UpdateAnswer` (crates/moqt/src/modules/moqt/version/update_answer.rs).
// - `outStart`: `send_request_update` with the Request ID reservation held
//   (the reservation serializes updates): holds the request stream
//   (`StreamSender::queue`) and, when PUSH_BEFORE_WRITE, queues the answer
//   waiter with `UpdateWaiters::enqueue` (`push_back`, one
//   `std::sync::Mutex` hold). A caller dropped before it holds the stream
//   has not started.
// - `outWriteSome` / `outWriteDone` / `outWriteFail`: the
//   `stream.sender.send(&message).await`. quinn's `write_all` can write part
//   of the message and then wait for flow control. A failed write runs
//   `QueuedUpdate::withdraw_unwritten` (`pop_back`, the reservation keeps it
//   last); a finished one consumes the Request ID and turns the
//   `QueuedUpdate` `written` into an `UpdateAnswer`.
// - `outPush`: only when PUSH_BEFORE_WRITE is off: the waiter is pushed
//   after the write, as a separate lock hold.
// - `outCancel`: the caller's future is dropped inside the send await,
//   dropping the `QueuedUpdate`. The waiter stays queued. With
//   WRITE_DETACHED (`QueuedMessage::write` runs the write in a task of its
//   own and the Request ID is consumed when it starts) the update still
//   reaches the peer whole; without it, the reservation is released
//   unconsumed and bytes already written stay on the stream (a torn
//   message).
// - `outTimeout`: `UpdateAnswer::wait` gives up after the control message
//   timeout and drops its receiver; the waiter stays queued.
// - `peerAnswer`: the peer answers its oldest unanswered update.
// - `outReceiveAnswer`: the request stream's reader runs `answer_update`
//   (`UpdateWaiters::answer_front`: `pop_front` under the waiters lock) for
//   a REQUEST_OK / REQUEST_ERROR; an answer with no waiter is ignored.
// Updates carry a ghost index (their order of issue) so the model can tell
// which update an answer belongs to.
//
// Inbound: the peer sends REQUEST_UPDATEs on a stream this side serves
// (crates/moqt/src/modules/moqt/version/draft18/request_streams.rs
// `IncomingRequestTask::serve` / `OutgoingRequestTask::read`, `UpdateAnswers`
// over `FuturesOrdered`; crates/moqt/src/modules/moqt/control_plane/handler/
// update_answer.rs `AnswerSlot`, `UpdateAnswerReceiver::within`).
// - `inArrive`: the reader pushes the update's answer future.
// - `inAppAnswer` / `inAppDrop`: `UpdateAnswerSlot::answer` under the slot
//   lock, or every handler clone dropped.
// - `inTimeout`: `within` times out and, under the slot lock, turns a
//   `Waiting` slot `TimedOut` (REQUEST_ERROR TIMEOUT).
// - `inWrite`: the select loop takes the next ready answer future and
//   writes its REQUEST_OK / REQUEST_ERROR.
//
// `draft18_request_update_current` is the implementation, with
// WRITE_DETACHED on (the write runs to completion even when the caller drops
// its future), and passes.
// The other instances turn off one guard the implementation relies on:
// - `draft18_request_update_push_after_write`: PUSH_BEFORE_WRITE off fails
//   `outAnswerReachesItsUpdate` (the answer can beat the push).
// - `draft18_request_update_unordered`: answers written as they become ready
//   (FuturesUnordered) fail `inAnswersInArrivalOrder`.
// - `draft18_request_update_racy_timeout`: a timeout that does not check the
//   slot under its lock fails `inAppToldWhatPeerGets`.
//
// Check (from docs/architecture/moqt/models, Java 21+):
//   quint typecheck draft18_request_update.qnt
//   quint verify --backend tlc --main draft18_request_update_current --invariant safety draft18_request_update.qnt   # passes
//   quint verify --backend tlc --main draft18_request_update_push_after_write --invariant safety draft18_request_update.qnt
//   quint verify --backend tlc --main draft18_request_update_unordered --invariant safety draft18_request_update.qnt
//   quint verify --backend tlc --main draft18_request_update_racy_timeout --invariant safety draft18_request_update.qnt
//   quint verify --backend tlc --main draft18_request_update_without_write_detached --invariant safety draft18_request_update.qnt
module draft18_request_update {
  // Number of REQUEST_UPDATEs each side sends on the stream.
  const N_OUT: int
  const N_IN: int
  const PUSH_BEFORE_WRITE: bool
  const WRITE_DETACHED: bool
  const ORDERED_ANSWERS: bool
  const ATOMIC_TIMEOUT: bool

  // One REQUEST_UPDATE this side sends, as its caller sees it.
  type OutUpdate =
    | OIdle
    | OQueued     // reservation held, waiter pushed (when PUSH_BEFORE_WRITE), nothing written
    | OPartial    // part of the message written, waiting for flow control
    | OPushing    // written, waiter not pushed yet (PUSH_BEFORE_WRITE off)
    | OWaiting    // written; `UpdateAnswer::wait` holds the receiver
    | OFailed     // the write failed; the caller got the error
    | OCancelled  // the caller dropped the send future
    | OTimedOut   // the caller's wait timed out and dropped the receiver
    | OGot(int)   // the caller received the answer the peer sent for update i

  // The answer slot of one REQUEST_UPDATE the peer sent (`AnswerSlot`).
  type InSlot =
    | SNotArrived
    | SWaiting
    | SAnswered
    | SDropped    // every handler clone dropped: answered REQUEST_OK
    | STimedOut   // answered REQUEST_ERROR TIMEOUT

  // What `UpdateAnswerSlot::answer` told the application.
  type Told = TNothing | TDelivered | TTimedOut

  type Answer = { update: int, ok: bool }

  type State = {
    out: int -> OutUpdate,
    // `update_waiters`, oldest first, as ghost update indices.
    waiters: List[int],
    // Updates the peer received and has not answered, in wire order.
    peerUnanswered: List[int],
    // Answers in flight to this side, in wire order.
    answersInFlight: List[int],
    // Some update's bytes were cut off mid-message on the stream.
    torn: bool,
    slots: int -> InSlot,
    told: int -> Told,
    // Pending answer futures, in arrival order.
    pendingAnswers: List[int],
    // Answers written on the stream, in order.
    written: List[Answer],
  }

  var s: State

  // Elements of the lists it is used on are distinct.
  pure def without(l: List[int], x: int): List[int] = l.select(y => y != x)

  action init = s' = {
    out: 1.to(N_OUT).mapBy(_ => OIdle),
    waiters: List(),
    peerUnanswered: List(),
    answersInFlight: List(),
    torn: false,
    slots: 1.to(N_IN).mapBy(_ => SNotArrived),
    told: 1.to(N_IN).mapBy(_ => TNothing),
    pendingAnswers: List(),
    written: List(),
  }

  // ---- Outbound ----

  def reservationHeld: bool =
    1.to(N_OUT).exists(k => match s.out.get(k) {
      | OQueued => true
      | OPartial => true
      | OPushing => true
      | _ => false
    })

  def isIdle(k: int): bool = match s.out.get(k) { | OIdle => true | _ => false }

  action outStart(k: int): bool = all {
    isIdle(k),
    1.to(N_OUT).forall(j => j < k implies not(isIdle(j))),
    not(reservationHeld),
    s' = {
      ...s,
      out: s.out.set(k, OQueued),
      waiters: if (PUSH_BEFORE_WRITE) s.waiters.append(k) else s.waiters,
    },
  }

  action outWriteSome(k: int): bool = all {
    s.out.get(k) == OQueued,
    s' = { ...s, out: s.out.set(k, OPartial) },
  }

  def written(k: int): State = {
    ...s,
    out: s.out.set(k, if (PUSH_BEFORE_WRITE) OWaiting else OPushing),
    peerUnanswered: s.peerUnanswered.append(k),
  }

  action outWriteDone(k: int): bool = all {
    s.out.get(k) == OQueued or s.out.get(k) == OPartial,
    s' = written(k),
  }

  action outWriteFail(k: int): bool = all {
    s.out.get(k) == OQueued,
    s' = {
      ...s,
      out: s.out.set(k, OFailed),
      waiters: if (PUSH_BEFORE_WRITE) s.waiters.slice(0, s.waiters.length() - 1) else s.waiters,
    },
  }

  action outPush(k: int): bool = all {
    s.out.get(k) == OPushing,
    s' = { ...s, out: s.out.set(k, OWaiting), waiters: s.waiters.append(k) },
  }

  // The caller drops its future inside the send await. With WRITE_DETACHED
  // the write goes on without it, so the update reaches the peer whole and
  // its (now receiverless) waiter keeps its place.
  action outCancel(k: int): bool = all {
    s.out.get(k) == OQueued or s.out.get(k) == OPartial,
    if (WRITE_DETACHED)
      s' = {
        ...s,
        out: s.out.set(k, OCancelled),
        peerUnanswered: s.peerUnanswered.append(k),
        waiters: if (PUSH_BEFORE_WRITE) s.waiters else s.waiters.append(k),
      }
    else
      s' = {
        ...s,
        out: s.out.set(k, OCancelled),
        torn: s.torn or s.out.get(k) == OPartial,
      },
  }

  action outTimeout(k: int): bool = all {
    s.out.get(k) == OWaiting,
    s' = { ...s, out: s.out.set(k, OTimedOut) },
  }

  action peerAnswer: bool = all {
    s.peerUnanswered.length() > 0,
    s' = {
      ...s,
      peerUnanswered: s.peerUnanswered.tail(),
      answersInFlight: s.answersInFlight.append(s.peerUnanswered.head()),
    },
  }

  action outReceiveAnswer: bool = all {
    s.answersInFlight.length() > 0,
    val a = s.answersInFlight.head()
    if (s.waiters.length() == 0)
      s' = { ...s, answersInFlight: s.answersInFlight.tail() }
    else
      val w = s.waiters.head()
      s' = {
        ...s,
        answersInFlight: s.answersInFlight.tail(),
        waiters: s.waiters.tail(),
        out: if (s.out.get(w) == OWaiting) s.out.set(w, OGot(a)) else s.out,
      },
  }

  // ---- Inbound ----

  action inArrive(j: int): bool = all {
    s.slots.get(j) == SNotArrived,
    1.to(N_IN).forall(i => i < j implies s.slots.get(i) != SNotArrived),
    s' = {
      ...s,
      slots: s.slots.set(j, SWaiting),
      pendingAnswers: s.pendingAnswers.append(j),
    },
  }

  action inAppAnswer(j: int): bool = all {
    s.told.get(j) == TNothing,
    s.slots.get(j) != SNotArrived,
    s.slots.get(j) != SDropped,
    if (s.slots.get(j) == SWaiting)
      s' = { ...s, slots: s.slots.set(j, SAnswered), told: s.told.set(j, TDelivered) }
    else if (s.slots.get(j) == STimedOut)
      s' = { ...s, told: s.told.set(j, TTimedOut) }
    else
      // Answered by an earlier call: `AlreadyAnswered`.
      s' = { ...s, told: s.told.set(j, TDelivered) },
  }

  action inAppDrop(j: int): bool = all {
    s.slots.get(j) == SWaiting,
    s.told.get(j) == TNothing,
    s' = { ...s, slots: s.slots.set(j, SDropped) },
  }

  // Without ATOMIC_TIMEOUT the timeout overrides an answer the slot already
  // took, as if it did not look at the slot under its lock.
  action inTimeout(j: int): bool = all {
    s.pendingAnswers.indices().exists(i => s.pendingAnswers[i] == j),
    if (ATOMIC_TIMEOUT) s.slots.get(j) == SWaiting
    else s.slots.get(j) == SWaiting or s.slots.get(j) == SAnswered,
    s' = { ...s, slots: s.slots.set(j, STimedOut) },
  }

  def ready(j: int): bool = s.slots.get(j) != SWaiting

  action inWrite: bool = all {
    s.pendingAnswers.length() > 0,
    val candidates =
      if (ORDERED_ANSWERS) Set(s.pendingAnswers.head())
      else s.pendingAnswers.indices().map(i => s.pendingAnswers[i])
    nondet j = oneOf(candidates)
    all {
      ready(j),
      s' = {
        ...s,
        pendingAnswers: without(s.pendingAnswers, j),
        written: s.written.append({ update: j, ok: s.slots.get(j) != STimedOut }),
      },
    },
  }

  action step = any {
    nondet k = oneOf(1.to(N_OUT))
    any {
      outStart(k),
      outWriteSome(k),
      outWriteDone(k),
      outWriteFail(k),
      outPush(k),
      outCancel(k),
      outTimeout(k),
    },
    peerAnswer,
    outReceiveAnswer,
    nondet j = oneOf(1.to(N_IN))
    any {
      inArrive(j),
      inAppAnswer(j),
      inAppDrop(j),
      inTimeout(j),
    },
    inWrite,
  }

  // Outbound: a caller that receives an answer receives the one the peer
  // sent for its own update (§10.9: answers come in update order).
  val outAnswerReachesItsUpdate: bool =
    1.to(N_OUT).forall(k => match s.out.get(k) {
      | OGot(a) => a == k
      | _ => true
    })

  // Outbound: no REQUEST_UPDATE is cut off mid-message, which the peer would
  // read as a malformed message and close the session for.
  val noTornUpdate: bool = not(s.torn)

  // Inbound: answers leave in the order the updates arrived, exactly one
  // each (§10.9).
  val inAnswersInArrivalOrder: bool =
    s.written.indices().forall(i => s.written[i].update == i + 1)

  // Inbound: an application told its answer was delivered sees it reach the
  // peer as REQUEST_OK; one told it timed out sees REQUEST_ERROR.
  val inAppToldWhatPeerGets: bool =
    s.written.indices().forall(i =>
      val a = s.written[i]
      and {
        s.told.get(a.update) == TDelivered implies a.ok,
        s.told.get(a.update) == TTimedOut implies not(a.ok),
      })

  // `outAnswerReachesItsUpdate` restricted to runs without a torn message,
  // where the peer reads every update whole: a cancel before any byte is
  // written still shifts every later answer by one.
  val misattributionNeedsNoTear: bool = s.torn or outAnswerReachesItsUpdate

  val safety: bool = and {
    outAnswerReachesItsUpdate,
    noTornUpdate,
    inAnswersInArrivalOrder,
    inAppToldWhatPeerGets,
  }
}

// expect: ok
module draft18_request_update_current {
  import draft18_request_update(
    N_OUT = 3, N_IN = 3,
    PUSH_BEFORE_WRITE = true, WRITE_DETACHED = true,
    ORDERED_ANSWERS = true, ATOMIC_TIMEOUT = true,
  ).*
}


// expect: violation
module draft18_request_update_push_after_write {
  import draft18_request_update(
    N_OUT = 3, N_IN = 3,
    PUSH_BEFORE_WRITE = false, WRITE_DETACHED = true,
    ORDERED_ANSWERS = true, ATOMIC_TIMEOUT = true,
  ).*
}

// expect: violation
module draft18_request_update_unordered {
  import draft18_request_update(
    N_OUT = 3, N_IN = 3,
    PUSH_BEFORE_WRITE = true, WRITE_DETACHED = true,
    ORDERED_ANSWERS = false, ATOMIC_TIMEOUT = true,
  ).*
}

// expect: violation
module draft18_request_update_racy_timeout {
  import draft18_request_update(
    N_OUT = 3, N_IN = 3,
    PUSH_BEFORE_WRITE = true, WRITE_DETACHED = true,
    ORDERED_ANSWERS = true, ATOMIC_TIMEOUT = false,
  ).*
}

// expect: violation
module draft18_request_update_without_write_detached {
  import draft18_request_update(
    N_OUT = 3, N_IN = 3,
    PUSH_BEFORE_WRITE = true, WRITE_DETACHED = false,
    ORDERED_ANSWERS = true, ATOMIC_TIMEOUT = true,
  ).*
}

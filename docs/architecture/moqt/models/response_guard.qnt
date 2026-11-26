// A model of `ResponseGuard`
// (crates/moqt/src/modules/moqt/control_plane/handler/response_guard.rs),
// which every request handler (`SubscribeHandler`, `FetchHandler`,
// `PublishHandler`, `TrackStatusHandler`, `PublishNamespaceHandler`,
// `SubscribeNamespaceHandler`) holds and clones with `#[derive(Clone)]`.
//
// One request; its handler clones are actors that may run on different
// threads. Each action is one atomic step:
// - `cloneHandler`: `Clone` adds a strong reference to `answer`.
// - `ok` / `error`: `accept` / `reject` (`&self`, callable any number of
//   times): `mark_responded` (`Answer::Answered`), then the response is
//   sent. Merged into one step: another clone's `Drop` between the two sees
//   `Answered` already.
// - `dropCheck`: `Drop::drop` calls `auto_reject_if_last`, which reads
//   `answer` and `Arc::strong_count` and decides whether to spawn the
//   NOT_SUPPORTED rejecter (`Answer::AutoRejected`).
// - `dropField`: after `Drop::drop` returns, the drop glue drops the
//   `answer` field, decrementing the strong count; a spawned rejecter
//   sends NOT_SUPPORTED. Two clones dropped on two threads can both run
//   `dropCheck` before either runs `dropField`.
//
// `response_guard_current` is the implementation; it violates both
// invariants. `response_guard_fixed` switches on ANSWER_ONCE and
// LAST_DROP_ATOMIC and passes; `response_guard_answer_once` switches on only
// ANSWER_ONCE and still fails `everyRequestAnswered`.
//
// Check (from docs/architecture/moqt/models, Java 21+):
//   quint typecheck response_guard.qnt
//   quint verify --backend tlc --main response_guard_current --invariant safety response_guard.qnt
//   quint verify --backend tlc --main response_guard_fixed --invariant safety response_guard.qnt
//   quint verify --backend tlc --main response_guard_answer_once --invariant safety response_guard.qnt
module response_guard {
  const CLONES: Set[str]
  // The clone the session event hands to the application.
  const FIRST: str
  // Fix J: `accept` / `reject` send only when `answer` was `Unanswered`.
  const ANSWER_ONCE: bool
  // Fix K: the last strong reference's drop itself decides (e.g. a `Drop` on
  // the `Arc`'s inner value), so the check and the decrement are one step.
  const LAST_DROP_ATOMIC: bool

  type Clone =
    | NotCloned
    | Live
    | Checked(bool)   // `Drop::drop` ran; true when it spawned the rejecter
    | Dropped

  type Response = Ok | Error | NotSupported

  type State = {
    clones: str -> Clone,
    // `ResponseGuard::answer != Unanswered`
    responded: bool,
    // `Arc::strong_count(&answer)`
    strongCount: int,
    // Responses sent for the request, in order.
    sent: List[Response],
  }

  var s: State

  action init = s' = {
    clones: CLONES.mapBy(g => if (g == FIRST) Live else NotCloned),
    responded: false,
    strongCount: 1,
    sent: List(),
  }

  action cloneHandler(original: str, copy: str): bool = all {
    s.clones.get(original) == Live,
    s.clones.get(copy) == NotCloned,
    s' = { ...s, clones: s.clones.set(copy, Live), strongCount: s.strongCount + 1 },
  }

  // `accept` (ok) or `reject` (error); the application may call either on
  // any clone, more than once. Bounded to keep the state space finite.
  action respond(g: str, r: Response): bool = all {
    s.clones.get(g) == Live,
    s.sent.length() < 3,
    if (ANSWER_ONCE and s.responded) s' = s
    else s' = { ...s, responded: true, sent: s.sent.append(r) },
  }

  def rejects: bool = not(s.responded) and s.strongCount == 1

  // The dropped clone's `answer` reference goes; a spawned rejecter
  // sends NOT_SUPPORTED.
  def released(g: str, spawned: bool): State = {
    ...s,
    clones: s.clones.set(g, Dropped),
    strongCount: s.strongCount - 1,
    sent: if (spawned) s.sent.append(NotSupported) else s.sent,
  }

  action dropCheck(g: str): bool = all {
    s.clones.get(g) == Live,
    if (LAST_DROP_ATOMIC) s' = released(g, rejects)
    else s' = { ...s, clones: s.clones.set(g, Checked(rejects)) },
  }

  action dropField(g: str): bool = any {
    all { s.clones.get(g) == Checked(true), s' = released(g, true) },
    all { s.clones.get(g) == Checked(false), s' = released(g, false) },
  }

  action step = {
    nondet g = oneOf(CLONES)
    nondet h = oneOf(CLONES)
    nondet r = oneOf(Set(Ok, Error))
    any {
      cloneHandler(g, h),
      respond(g, r),
      dropCheck(g),
      dropField(g),
    }
  }

  // The requester receives at most one response (a second one for the same
  // Request ID makes it close the session).
  val atMostOneResponse: bool = s.sent.length() <= 1

  // Once every clone is gone, the request has been answered: the requester
  // never waits for its timeout.
  val everyRequestAnswered: bool =
    CLONES.forall(g => s.clones.get(g) == Dropped or s.clones.get(g) == NotCloned)
      implies s.sent.length() >= 1

  val safety: bool = and {
    atMostOneResponse,
    everyRequestAnswered,
  }
}

// expect: violation
module response_guard_current {
  import response_guard(
    CLONES = Set("g1", "g2"),
    FIRST = "g1",
    ANSWER_ONCE = false,
    LAST_DROP_ATOMIC = false,
  ).*
}

// expect: violation
module response_guard_answer_once {
  import response_guard(
    CLONES = Set("g1", "g2"),
    FIRST = "g1",
    ANSWER_ONCE = true,
    LAST_DROP_ATOMIC = false,
  ).*
}

// expect: ok
module response_guard_fixed {
  import response_guard(
    CLONES = Set("g1", "g2"),
    FIRST = "g1",
    ANSWER_ONCE = true,
    LAST_DROP_ATOMIC = true,
  ).*
}

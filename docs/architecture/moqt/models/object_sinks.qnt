// A model of the per-track-alias object sinks of one connection
// (`ObjectSinks` in crates/moqt/src/modules/moqt/domains/connection.rs).
//
// Each action is one critical section: what runs while
// `Connection::object_sinks` (a tokio mutex) is locked.
// - `arrive`: `ObjectSinks::notify`, called by `subscription_notifier::notify`
//   for every subgroup stream header and datagram.
// - `install`: `ObjectSinks::install`, called by
//   `SessionContext::register_data_receiver` after SUBSCRIBE_OK
//   (`Subscriber::subscribe_joining`) or when the application accepts a
//   PUBLISH (`PublishHandler::accept_data_receiver`).
// - `cancelSubscription`: `ObjectSinks::cancel_subscription` (UNSUBSCRIBE,
//   PUBLISH_DONE expiry, abandoned SUBSCRIBE_OK).
// - `cancelUnclaimed`: `ObjectSinks::cancel_unclaimed` (PUBLISH_ERROR,
//   abandoned SUBSCRIBE_OK whose alias was never installed).
// - `dropReader`: the application drops the `DataReceiver`; the sink keeps
//   its sender and answers `ReceiverClosed`.
// - `tick`: `executor::now()` advances by one unit.
//
// Time is scaled: `PERIOD` ticks stand for UNKNOWN_TRACK_ALIAS_PERIOD (10 s),
// `PER_ALIAS` for MAX_PENDING_OBJECTS_PER_TRACK_ALIAS (256) and `TOTAL` for
// MAX_PENDING_OBJECTS (1024). The `expiries` queue is not modeled: it is
// ordered by `created_at`, holds one entry per Buffer/Cancelled sink ever
// created, and `remove_expired` pops every entry whose period ended, so it
// removes exactly the Buffer/Cancelled sinks with `createdAt + PERIOD <= now`.
//
// Switches:
// - EXPIRE_ON_INSTALL (fix): `install` runs `remove_expired` first, as
//   `notify` and `cancel` do. Off, a buffer whose period ended while the
//   connection received nothing is drained into a new receiver
//   (`noStaleDrain`).
// - CHECK_TOTAL (guard the code relies on): `notify` refuses a new object
//   once the connection buffers TOTAL (`bufferBounded`).
//
// Check (from docs/architecture/moqt/models, Java 21+):
//   quint typecheck object_sinks.qnt
//   quint verify --backend tlc --main object_sinks_current --invariant safety object_sinks.qnt    # passes
//   quint verify --backend tlc --main object_sinks_fixed --invariant safety object_sinks.qnt      # passes
//   quint verify --backend tlc --main object_sinks_no_total_check --invariant safety object_sinks.qnt  # violates bufferBounded
module object_sinks {
  const ALIASES: Set[str]
  const PERIOD: int
  const MAX_TIME: int
  const PER_ALIAS: int
  const TOTAL: int
  const EXPIRE_ON_INSTALL: bool
  const CHECK_TOTAL: bool

  // `ObjectSink`, plus `Absent` for a track alias without an entry.
  type Sink =
    | Absent
    // Objects of an alias no subscription established yet; `objects` holds
    // their arrival times, oldest first.
    | Buffer({ createdAt: int, objects: List[int] })
    // `readerOpen` is false once the application dropped its DataReceiver.
    | Receiver({ readerOpen: bool })
    // A cancelled subscription's alias: late objects are discarded.
    | Cancelled(int)

  var sinks: str -> Sink
  // `ObjectSinks::pending_objects`
  var pending: int
  var now: int
  // Set when `install` hands the new receiver an object buffered under an
  // alias whose UNKNOWN_TRACK_ALIAS_PERIOD had already ended.
  var staleDrain: bool
  // Set when `install` found another receiver (DUPLICATE_TRACK_ALIAS closes
  // the session).
  var duplicate: bool

  action init = all {
    sinks' = ALIASES.mapBy(_ => Absent),
    pending' = 0,
    now' = 0,
    staleDrain' = false,
    duplicate' = false,
  }

  def expired(sink: Sink): bool = match sink {
    | Buffer(b) => b.createdAt + PERIOD <= now
    | Cancelled(createdAt) => createdAt + PERIOD <= now
    | _ => false
  }

  def bufferedIn(sink: Sink): int = match sink {
    | Buffer(b) => b.objects.length()
    | _ => 0
  }

  // `remove_expired`, with `release` adjusting `pending_objects`.
  def withoutExpired(m: str -> Sink): str -> Sink =
    m.keys().mapBy(a => if (expired(m.get(a))) Absent else m.get(a))

  def releasedBy(m: str -> Sink): int =
    m.keys().fold(0, (acc, a) => if (expired(m.get(a))) acc + bufferedIn(m.get(a)) else acc)

  def isReceiver(sink: Sink): bool = match sink {
    | Receiver(_) => true
    | _ => false
  }

  action tick = all {
    now < MAX_TIME,
    now' = now + 1,
    sinks' = sinks, pending' = pending, staleDrain' = staleDrain, duplicate' = duplicate,
  }

  // `notify`
  action arrive(a: str): bool = {
    val live = withoutExpired(sinks)
    val p = pending - releasedBy(sinks)
    val sink = live.get(a)
    val full = CHECK_TOTAL and p >= TOTAL
    all {
      staleDrain' = staleDrain, duplicate' = duplicate, now' = now,
      match sink {
        | Absent =>
          if (full) all { sinks' = live, pending' = p }
          else all {
            sinks' = live.set(a, Buffer({ createdAt: now, objects: [now] })),
            pending' = p + 1,
          }
        | Buffer(b) =>
          if (b.objects.length() >= PER_ALIAS)
            // drop the oldest
            all {
              sinks' = live.set(a, Buffer({ ...b, objects: b.objects.tail().append(now) })),
              pending' = p,
            }
          else if (full) all { sinks' = live, pending' = p }
          else all {
            sinks' = live.set(a, Buffer({ ...b, objects: b.objects.append(now) })),
            pending' = p + 1,
          }
        // `deliver`: a closed reader answers ReceiverClosed; nothing changes.
        | Receiver(_) => all { sinks' = live, pending' = p }
        | Cancelled(_) => all { sinks' = live, pending' = p }
      },
    }
  }

  // `install`
  action install(a: str): bool = {
    val live = if (EXPIRE_ON_INSTALL) withoutExpired(sinks) else sinks
    val p = if (EXPIRE_ON_INSTALL) pending - releasedBy(sinks) else pending
    val sink = live.get(a)
    all {
      now' = now,
      if (isReceiver(sink)) all {
        duplicate' = true, sinks' = live, pending' = p, staleDrain' = staleDrain,
      } else all {
        duplicate' = duplicate,
        sinks' = live.set(a, Receiver({ readerOpen: true })),
        pending' = p - bufferedIn(sink),
        staleDrain' = (staleDrain or match sink {
          | Buffer(b) => b.objects.length() > 0 and b.createdAt + PERIOD <= now
          | _ => false
        }),
      },
    }
  }

  // `cancel`
  action cancelAt(a: str): bool = {
    val live = withoutExpired(sinks)
    val p = pending - releasedBy(sinks)
    all {
      sinks' = live.set(a, Cancelled(now)),
      pending' = p - bufferedIn(live.get(a)),
      now' = now, staleDrain' = staleDrain, duplicate' = duplicate,
    }
  }

  action cancelSubscription(a: str): bool = all { isReceiver(sinks.get(a)), cancelAt(a) }

  action cancelUnclaimed(a: str): bool = all { not(isReceiver(sinks.get(a))), cancelAt(a) }

  action dropReader(a: str): bool = all {
    sinks.get(a) == Receiver({ readerOpen: true }),
    sinks' = sinks.set(a, Receiver({ readerOpen: false })),
    pending' = pending, now' = now, staleDrain' = staleDrain, duplicate' = duplicate,
  }

  action step = any {
    tick,
    nondet a = oneOf(ALIASES)
    any {
      arrive(a),
      install(a),
      cancelSubscription(a),
      cancelUnclaimed(a),
      dropReader(a),
    },
  }

  // `pending_objects` counts exactly the buffered objects.
  val pendingCounted: bool =
    pending == ALIASES.fold(0, (acc, a) => acc + bufferedIn(sinks.get(a)))

  // A connection buffers at most PER_ALIAS objects per unknown track alias
  // and TOTAL across all of them.
  val bufferBounded: bool = and {
    pending <= TOTAL,
    ALIASES.forall(a => bufferedIn(sinks.get(a)) <= PER_ALIAS),
  }

  // A subscription never receives objects that were buffered under its
  // alias by a buffer whose UNKNOWN_TRACK_ALIAS_PERIOD had ended before the
  // alias was established (draft-14 §10.3: buffered "for a brief period").
  val noStaleDrain: bool = not(staleDrain)

  // A cancelled alias discards objects only for PERIOD: an object arriving
  // later is buffered as unknown again (draft-14 §10.1).
  val cancelledIsFresh: bool =
    ALIASES.forall(a => match sinks.get(a) {
      | Cancelled(createdAt) => createdAt <= now
      | _ => true
    })

  val safety: bool = and {
    pendingCounted,
    bufferBounded,
    noStaleDrain,
    cancelledIsFresh,
  }
}

// expect: ok
module object_sinks_current {
  import object_sinks(
    ALIASES = Set("a1", "a2"),
    PERIOD = 2,
    MAX_TIME = 3,
    PER_ALIAS = 2,
    TOTAL = 3,
    EXPIRE_ON_INSTALL = true,
    CHECK_TOTAL = true,
  ).*
}

// expect: ok
module object_sinks_fixed {
  import object_sinks(
    ALIASES = Set("a1", "a2"),
    PERIOD = 2,
    MAX_TIME = 3,
    PER_ALIAS = 2,
    TOTAL = 3,
    EXPIRE_ON_INSTALL = true,
    CHECK_TOTAL = true,
  ).*
}

// expect: violation
module object_sinks_no_total_check {
  import object_sinks(
    ALIASES = Set("a1", "a2"),
    PERIOD = 2,
    MAX_TIME = 3,
    PER_ALIAS = 2,
    TOTAL = 3,
    EXPIRE_ON_INSTALL = true,
    CHECK_TOTAL = false,
  ).*
}

// A model of the group state of one `TrackWriter`
// (crates/moqt/src/modules/moqt/domains/track_writer.rs) as an Origin outlet
// drives it (origin/outlet_send_task.rs), and of the Prior Group ID Gap
// (draft-ietf-moq-transport-14 §11.1, draft-18 §12.8) it puts on objects.
//
// One action is one `Origin::write` / `Origin::write_group` (origin.rs
// :504-570): the numbering places the object (origin/object_numbering.rs
// `place`), `queue_sends` (origin.rs :986-1000) queues it on the outlet, and
// the outlet's task runs the command to completion. The task is sequential
// and gets commands in numbering order, so running each command at once
// loses no interleaving that matters to the writer; the Forward State only
// matters at the moment `queue_sends` reads it.
// - Forward State 1: `send_placed` (outlet_send_task.rs :171-206) →
//   `start_group_at` (:86) = `GroupNumbering::skip_to` (:320-335) +
//   `finish_current_group` + `open_group` (:216-243); or `rejoin_group_at`
//   (:63-73); or `join_group_at` (:94-104); then
//   `write_with_extension_headers` (:130-144), which takes the pending gap.
//   `Payload::Group` → `write_group_at` (:106-113) → `write_group` (:146-152).
// - Forward State 0: `LeaveGroup` → `abandon_group` (:52-54).
// - `open_group` may fail (no stream, or setting priority / the header
//   fails); a write may fail (STOP_SENDING, reset). The outlet's writer is
//   created with first group id 0 (origin.rs `outlet_send_task` :208-221).
//
// Prior Group ID Gap = N on an object of group G says groups G-N..G-1 "do
// not and will never exist"; a receiver treats the track as malformed when a
// gap covers an object it received or an object arrives in a communicated
// gap, and when a group carries two different gap values.
//
// Switches:
// - FORWARD_TOGGLES: the subscriber changes the Forward State.
// - OPEN_CAN_FAIL / WRITE_CAN_FAIL: transport failures.
// - ADVANCE_ON_OPEN_FAILURE (partial fix): when opening a group's stream
//   fails, `open_group` still moves `next` past the group and drops
//   the pending gap, which no longer ends right before the next group.
// - GAP_FROM_NUMBERING (fix): the writer stops deriving the gap from its own
//   `next`; the outlet puts on a group's first object the gap the
//   numbering made (`GroupBoundary::At` past the next id), the only groups
//   the origin knows never exist.
//
// Commands (from docs/architecture/moqt/models, Java 21+ on PATH):
//   quint typecheck track_writer.qnt
//   quint run --main track_writer_current --invariant safety --max-samples 5000 --max-steps 30 track_writer.qnt
//   quint verify --backend tlc --main track_writer_current --invariant safety track_writer.qnt
//   (likewise for track_writer_always_forward, track_writer_advance_on_open_failure,
//   track_writer_advance_on_open_failure_with_forward, track_writer_fixed)
module track_writer {
  const MAX_GROUP: int
  const MAX_OBJECT: int
  const FORWARD_TOGGLES: bool
  const OPEN_CAN_FAIL: bool
  const WRITE_CAN_FAIL: bool
  const ADVANCE_ON_OPEN_FAILURE: bool
  const GAP_FROM_NUMBERING: bool

  // `GroupSlot` (`NoGroup` / `Open` / `Abandoned`), by its group id.
  // `group` and `abandoned` below are its two non-empty variants.
  type Slot = NoGroup | Group(int)

  // `TrackWriter`'s `GroupNumbering` (`first`, `next`, `pending_gap`) and
  // `GroupSlot`. `pendingGap` 0 is `None` (`skip_to` never stores
  // `Some(0)`). `next` never reaches u64::MAX here.
  type Writer = {
    first: int,
    next: int,
    group: Slot,
    abandoned: Slot,
    pendingGap: int,
  }

  // What the subscriber received on the outlet's streams.
  type Receiver = {
    // Groups it received an object of.
    received: Set[int],
    // Groups a Prior Group ID Gap told it never exist.
    announced: Set[int],
    // (group, gap) of every object that carried the gap.
    gaps: Set[(int, int)],
  }

  type Sent = { w: Writer, r: Receiver }

  type Placement = { group: int, object: int, startsGroup: bool, gap: int }

  // `ObjectNumbering::largest`
  type Numbered = NoObject | Largest((int, int))

  var numbered: Numbered
  // Groups the numbering started: they exist (cached, FETCHable, sent to
  // other outlets).
  var placed: Set[int]
  var forward: bool
  var writer: Writer
  var receiver: Receiver

  pure def groups(w: Writer): int = w.next - w.first

  // `GroupNumbering::skip_to`; the caller never passes a group before `next`.
  pure def skipTo(w: Writer, g: int): Writer =
    if (groups(w) == 0) { ...w, first: g, next: g }
    else {
      ...w,
      next: g,
      pendingGap:
        if (not(GAP_FROM_NUMBERING) and g > w.next) g - w.next else w.pendingGap,
    }

  // `finish_current_group`
  pure def finishCurrent(w: Writer): Writer = { ...w, group: NoGroup, abandoned: NoGroup }

  // `open_group` at `next`; on failure the caller returns the error.
  pure def opened(w: Writer, fails: bool): Writer =
    if (fails and ADVANCE_ON_OPEN_FAILURE) { ...w, next: w.next + 1, pendingGap: 0 }
    else if (fails) w
    else { ...w, group: Group(w.next), next: w.next + 1 }

  // `write_with_extension_headers` on group `g`, carrying `gap` from the
  // numbering under GAP_FROM_NUMBERING.
  pure def written(w: Writer, r: Receiver, g: int, gap: int, fails: bool): Sent =
    val carried = if (GAP_FROM_NUMBERING) gap else w.pendingGap
    val w2 = { ...w, pendingGap: 0 }
    if (fails) { w: { ...w2, group: NoGroup }, r: r }
    else {
      w: w2,
      r: {
        received: r.received.union(Set(g)),
        announced:
          if (carried > 0) r.announced.union((g - carried).to(g - 1)) else r.announced,
        gaps: if (carried > 0) r.gaps.union(Set((g, carried))) else r.gaps,
      },
    }

  pure def openThenWrite(w: Writer, r: Receiver, p: Placement, openFails: bool, writeFails: bool): Sent =
    if (openFails) { w: opened(w, true), r: r }
    else written(opened(w, false), r, p.group, if (p.startsGroup) p.gap else 0, writeFails)

  // `send_placed`
  pure def sendPlaced(w: Writer, r: Receiver, p: Placement, openFails: bool, writeFails: bool): Sent =
    if (p.startsGroup)
      openThenWrite(finishCurrent(skipTo(w, p.group)), r, p, openFails, writeFails)
    else if (w.abandoned == Group(p.group))
      openThenWrite({ ...w, abandoned: NoGroup, next: p.group }, r, p, openFails, writeFails)
    else if (w.group != Group(p.group))
      if (groups(w) > 0) { w: w, r: r }
      else openThenWrite(skipTo(w, p.group), r, p, openFails, writeFails)
    else written(w, r, p.group, 0, writeFails)

  // `write_group_at`: one object as a whole group.
  pure def sendGroup(w: Writer, r: Receiver, p: Placement, openFails: bool, writeFails: bool): Sent =
    val w1 = finishCurrent(skipTo(w, p.group))
    if (openFails) { w: opened(w1, true), r: r }
    else {
      val sent = written(opened(w1, false), r, p.group, p.gap, writeFails)
      // The group is finished at once, never kept open.
      { w: { ...sent.w, group: NoGroup }, r: sent.r }
    }

  // `abandon_group`
  pure def abandoned(w: Writer): Writer = match w.group {
    | NoGroup => w
    | Group(_) => { ...w, group: NoGroup, abandoned: w.group }
  }

  pure def largestGroup(n: Numbered): int = match n {
    | NoObject => -1
    | Largest(l) => l._1
  }

  pure def largestObject(n: Numbered): int = match n {
    | NoObject => -1
    | Largest(l) => l._2
  }

  // `queue_sends` on the one outlet, then the outlet's task.
  action deliver(p: Placement, whole: bool, openFails: bool, writeFails: bool): bool = all {
    numbered' = Largest((p.group, p.object)),
    placed' = placed.union(Set(p.group)),
    forward' = forward,
    if (not(forward)) all {
      writer' = abandoned(writer),
      receiver' = receiver,
    } else {
      val sent =
        if (whole) sendGroup(writer, receiver, p, openFails, writeFails)
        else sendPlaced(writer, receiver, p, openFails, writeFails)
      all { writer' = sent.w, receiver' = sent.r }
    },
  }

  // `Origin::write` with `GroupBoundary::Within`.
  action writeWithin(openFails: bool, writeFails: bool): bool = all {
    numbered != NoObject,
    largestObject(numbered) < MAX_OBJECT,
    deliver(
      { group: largestGroup(numbered), object: largestObject(numbered) + 1, startsGroup: false, gap: 0 },
      false, openFails, writeFails),
  }

  // `Origin::write` with `GroupBoundary::At(g)` (`Next` is `At(largest + 1)`).
  action writeAt(g: int, openFails: bool, writeFails: bool): bool = all {
    g > largestGroup(numbered),
    val gap = if (numbered == NoObject) 0 else g - largestGroup(numbered) - 1
    deliver({ group: g, object: 0, startsGroup: true, gap: gap }, false, openFails, writeFails),
  }

  // `Origin::write_group`: `GroupBoundary::Next`, sent as a whole group.
  action writeGroup(openFails: bool, writeFails: bool): bool = all {
    largestGroup(numbered) < MAX_GROUP,
    deliver(
      { group: largestGroup(numbered) + 1, object: 0, startsGroup: true, gap: 0 },
      true, openFails, writeFails),
  }

  // SUBSCRIBE_UPDATE / REQUEST_UPDATE changing the Forward State.
  action toggleForward = all {
    FORWARD_TOGGLES,
    forward' = not(forward),
    numbered' = numbered,
    placed' = placed,
    writer' = writer,
    receiver' = receiver,
  }

  action init = all {
    numbered' = NoObject,
    placed' = Set(),
    forward' = true,
    writer' = { first: 0, next: 0, group: NoGroup, abandoned: NoGroup, pendingGap: 0 },
    receiver' = { received: Set(), announced: Set(), gaps: Set() },
  }

  action step = {
    nondet g = oneOf(0.to(MAX_GROUP))
    nondet openFails = oneOf(if (OPEN_CAN_FAIL) Set(true, false) else Set(false))
    nondet writeFails = oneOf(if (WRITE_CAN_FAIL) Set(true, false) else Set(false))
    any {
      writeWithin(openFails, writeFails),
      writeAt(g, openFails, writeFails),
      writeGroup(openFails, writeFails),
      toggleForward,
    }
  }

  // The origin never tells a subscriber that a group it has does not exist.
  val gapCoversOnlyMissingGroups: bool = receiver.announced.intersect(placed) == Set()

  // The subscriber never finds the track malformed by a gap: no gap covers
  // an object it received, and no object arrives in a communicated gap.
  val gapNeverCoversReceived: bool = receiver.announced.intersect(receiver.received) == Set()

  // No group carries two different Prior Group ID Gap values.
  val oneGapPerGroup: bool =
    receiver.gaps.forall(a => receiver.gaps.forall(b => a._1 == b._1 implies a._2 == b._2))

  // The writer never holds an open group and an abandoned one at once.
  val openOrAbandoned: bool = not(writer.group != NoGroup and writer.abandoned != NoGroup)

  val safety: bool = and {
    gapCoversOnlyMissingGroups,
    gapNeverCoversReceived,
    oneGapPerGroup,
    openOrAbandoned,
  }
}

// expect: violation
module track_writer_current {
  import track_writer(
    MAX_GROUP = 3,
    MAX_OBJECT = 1,
    FORWARD_TOGGLES = true,
    OPEN_CAN_FAIL = true,
    WRITE_CAN_FAIL = true,
    ADVANCE_ON_OPEN_FAILURE = false,
    GAP_FROM_NUMBERING = false,
  ).*
}

// Forward State always 1: the open-failure mechanism alone.
// expect: violation
module track_writer_always_forward {
  import track_writer(
    MAX_GROUP = 3,
    MAX_OBJECT = 1,
    FORWARD_TOGGLES = false,
    OPEN_CAN_FAIL = true,
    WRITE_CAN_FAIL = true,
    ADVANCE_ON_OPEN_FAILURE = false,
    GAP_FROM_NUMBERING = false,
  ).*
}

// expect: ok
module track_writer_advance_on_open_failure {
  import track_writer(
    MAX_GROUP = 3,
    MAX_OBJECT = 1,
    FORWARD_TOGGLES = false,
    OPEN_CAN_FAIL = true,
    WRITE_CAN_FAIL = true,
    ADVANCE_ON_OPEN_FAILURE = true,
    GAP_FROM_NUMBERING = false,
  ).*
}

// Advancing on open failure does not cover groups skipped in Forward State 0.
// expect: violation
module track_writer_advance_on_open_failure_with_forward {
  import track_writer(
    MAX_GROUP = 3,
    MAX_OBJECT = 1,
    FORWARD_TOGGLES = true,
    OPEN_CAN_FAIL = true,
    WRITE_CAN_FAIL = true,
    ADVANCE_ON_OPEN_FAILURE = true,
    GAP_FROM_NUMBERING = false,
  ).*
}

// expect: ok
module track_writer_fixed {
  import track_writer(
    MAX_GROUP = 3,
    MAX_OBJECT = 1,
    FORWARD_TOGGLES = true,
    OPEN_CAN_FAIL = true,
    WRITE_CAN_FAIL = true,
    ADVANCE_ON_OPEN_FAILURE = false,
    GAP_FROM_NUMBERING = true,
  ).*
}

// A model of `GroupCutover` (crates/moqt/src/modules/moqt/domains/group_cutover.rs):
// one track read on two connections while a session migrates, and which
// connection each object the application sees comes from.
//
// Code mapping:
// - `GroupCutover::{waiting_after, take_previous, take_current,
//   release_previous, previous_ended}` (group_cutover.rs:64-134).
// - `SinkCutover::takes` (domains/connection.rs:163-171) calls
//   `take_previous`/`take_current` under the `GroupCutover` mutex with
//   `group_of` (connection.rs:156-161): the group of a datagram or of a
//   subgroup stream, even one that starts mid-group. Before CUT_FROM_START
//   `take_current` also took the object ID, `0` for a subgroup stream.
// - `current_start` is `first_whole_group` (session/migration_task.rs:824-835):
//   the first group the new subscription delivers whole.
// - `release_when_idle` (session/migration_task.rs:639-662) calls
//   `release_previous` once no subgroup stream is open on the previous
//   connection; `Router::retire` (migration_task.rs:364-386) calls
//   `previous_ended` and closes the previous connection.
//
// One action = one hold of the `GroupCutover` mutex: one object (datagram)
// or one subgroup stream reaching a sink, one `release_previous` that
// returned true, or one `previous_ended`. A subgroup stream is modeled as
// arriving whole at once; `release_when_idle` only runs while no stream is
// open, so it never interleaves with one.
//
// The peers are nondeterministic: both connections deliver any object of the
// track in any order (streams and datagrams reorder), each at most once; the
// new subscription only delivers objects at or after its first location
// `first` (draft-14 §9.7: nothing up to SUBSCRIBE_OK's Largest Object).
//
// Instances:
// - group_cutover_{subgroups,datagrams}_current: today's code.
// - *_keep_newest: KEEP_NEWEST only (previous_ended keeps `newest`).
// - *_cut_from_start: CUT_FROM_START fixes the cutover at
//   max(newest + 1, current_start) instead of at the group of the first
//   object the current subscription happens to deliver.
// - *_fixed: CUT_FROM_START and KEEP_NEWEST (previous_ended keeps `newest`).
//
// Check (from docs/architecture/moqt/models):
//   quint typecheck group_cutover.qnt
//   quint run --main group_cutover_subgroups_current --invariant safety --max-samples 5000 --max-steps 12 group_cutover.qnt
//   quint verify --backend tlc --main group_cutover_subgroups_current --invariant safety group_cutover.qnt
//   (same for group_cutover_datagrams_current, *_cut_from_start, *_fixed)
module group_cutover {
  // Group IDs of the track.
  const GROUPS: Set[int]
  // Object IDs of every group.
  const OBJECTS: Set[int]
  // Objects arrive as datagrams (one decision per object) instead of
  // subgroup streams (one decision per group).
  const DATAGRAMS: bool
  // Fix: the cutover is max(newest + 1, current_start) whenever it is fixed.
  const CUT_FROM_START: bool
  // Fix: `previous_ended` keeps `newest`.
  const KEEP_NEWEST: bool

  // `Cutover` (group_cutover.rs:19).
  type Cutover = Waiting | At(int)

  // The previous connection's side of the subscription; the code's
  // `PreviousSide {Live, Ended}` (group_cutover.rs:32) while `Waiting`.
  type PreviousSide =
    | PLive      // delivering; the cutover may still be waiting for it
    | PReleased  // `release_previous` returned true
    | PEnded     // `retire` closed the connection and called `previous_ended`

  type Loc = (int, int)

  type State = {
    // `PreviousSide::Live::newest`; -1 for `None` and for `Ended`.
    newest: int,
    currentStart: int,
    cut: Cutover,
    previous: PreviousSide,
    // The new subscription's first location.
    first: Loc,
    // Objects that reached each connection's sink.
    prevSent: Set[Loc],
    curSent: Set[Loc],
    // Objects handed to the application, and from which connection.
    seen: Set[Loc],
    takenPrev: Set[int],
    takenCur: Set[int],
    duplicated: bool,
  }

  var s: State

  pure def before(a: Loc, b: Loc): bool =
    a._1 < b._1 or (a._1 == b._1 and a._2 < b._2)

  pure def groupObjects(objects: Set[int], g: int): Set[Loc] =
    objects.map(o => (g, o))

  def wholeGroup(sent: Set[Loc], g: int): bool =
    groupObjects(OBJECTS, g).subseteq(sent)

  // `first_whole_group`.
  pure def firstWholeGroup(first: Loc): int =
    if (first._2 == 0) first._1 else first._1 + 1

  def nextAfterNewest: int = if (s.newest < 0) 0 else s.newest + 1

  // The cutover `take_current` fixes while waiting, for an object of `g`.
  def waitingCut(g: int, o: int): int =
    if (CUT_FROM_START) {
      if (nextAfterNewest > s.currentStart) nextAfterNewest else s.currentStart
    } else if (s.newest >= 0 and g <= s.newest) s.newest + 1
    else if (o == 0 and g >= s.currentStart) g
    else g + 1

  pure def noted(newest: int, g: int): int = if (g > newest) g else newest

  // Hands `objects` of group `g` to the application when `take`.
  def handed(st: State, take: bool, objects: Set[Loc]): State =
    if (take) {
      ...st,
      seen: st.seen.union(objects),
      duplicated: st.duplicated or objects.intersect(st.seen) != Set(),
    }
    else st

  // `take_previous` under the `SharedCutover` lock.
  def takePrevious(g: int, objects: Set[Loc]): State =
    val take = match s.cut {
      | Waiting => true
      | At(c) => g < c
    }
    val st = {
      ...s,
      prevSent: s.prevSent.union(objects),
      newest: if (take) noted(s.newest, g) else s.newest,
      takenPrev: if (take) s.takenPrev.union(Set(g)) else s.takenPrev,
    }
    handed(st, take, objects)

  // `take_current` under the `SharedCutover` lock.
  def takeCurrent(g: int, o: int, objects: Set[Loc]): State =
    val c = match s.cut {
      | Waiting => waitingCut(g, o)
      | At(c) => c
    }
    val take = g >= c
    val st = {
      ...s,
      cut: At(c),
      curSent: s.curSent.union(objects),
      newest: if (take) noted(s.newest, g) else s.newest,
      takenCur: if (take) s.takenCur.union(Set(g)) else s.takenCur,
    }
    handed(st, take, objects)

  // The previous connection's peer delivers a datagram or a whole subgroup
  // stream; it delivers nothing once retired.
  action previousDelivers(g: int, o: int): bool = all {
    s.previous != PEnded,
    if (DATAGRAMS) all {
      not(s.prevSent.contains((g, o))),
      s' = takePrevious(g, Set((g, o))),
    } else all {
      o == 0,
      s.prevSent.filter(l => l._1 == g) == Set(),
      s' = takePrevious(g, groupObjects(OBJECTS, g)),
    },
  }

  // The new connection's peer delivers a datagram or a subgroup stream at or
  // after `first`; the stream of `first`'s group starts at `first`.
  action currentDelivers(g: int, o: int): bool = all {
    not(before((g, o), s.first)),
    if (DATAGRAMS) all {
      not(s.curSent.contains((g, o))),
      s' = takeCurrent(g, o, Set((g, o))),
    } else all {
      o == (if (g == s.first._1) s.first._2 else 0),
      s.curSent.filter(l => l._1 == g) == Set(),
      // `location_of` reports a subgroup stream as (group, 0).
      s' = takeCurrent(g, 0, OBJECTS.filter(x => x >= o).map(x => (g, x))),
    },
  }

  // `release_when_idle`: `release_previous` returned true. A false return
  // changes nothing and is not an action.
  action releasePrevious: bool = all {
    s.previous == PLive,
    match s.cut {
      | At(_) => s' = { ...s, previous: PReleased }
      | Waiting => all {
        not(s.newest >= 0 and s.newest + 1 < s.currentStart),
        val releasedCut =
          if (CUT_FROM_START and s.currentStart > nextAfterNewest) s.currentStart
          else nextAfterNewest
        s' = { ...s, previous: PReleased, cut: At(releasedCut) },
      }
    },
  }

  // `Router::retire` -> `previous_ended`: the grace timer, the peer closing
  // the previous connection, or `released` resolving after the release.
  action previousEnds: bool = all {
    s.previous != PEnded,
    s' = {
      ...s,
      previous: PEnded,
      newest: if (s.cut == Waiting and not(KEEP_NEWEST)) -1 else s.newest,
    },
  }

  // The previous subscription handed out whole groups up to `newest0`
  // before the migration. `first` follows `resumed_filter`: NextGroupStart
  // starts at a group start (the new relay may lag or lead), an
  // AbsoluteRange after `newest0` (draft-14 §9.7 may push it mid-group).
  action init = {
    nondet newest0 = Set(-1, 0, 1).oneOf()
    nondet fg = GROUPS.oneOf()
    nondet fo = OBJECTS.oneOf()
    all {
      fo == 0 or newest0 < 0 or fg > newest0,
      val before0 = tuples(GROUPS, OBJECTS).filter(l => l._1 <= newest0)
      s' = {
        newest: newest0,
        currentStart: firstWholeGroup((fg, fo)),
        cut: Waiting,
        previous: PLive,
        first: (fg, fo),
        prevSent: before0,
        curSent: Set(),
        seen: before0,
        takenPrev: GROUPS.filter(g => g <= newest0),
        takenCur: Set(),
        duplicated: false,
      },
    }
  }

  action step = any {
    releasePrevious,
    previousEnds,
    nondet g = GROUPS.oneOf()
    nondet o = OBJECTS.oneOf()
    any { previousDelivers(g, o), currentDelivers(g, o) },
  }

  // The application never gets the same object twice.
  val noDuplicate: bool = not(s.duplicated)

  // Once the cutover is fixed, every group the application got from the
  // previous connection lies before it and every group it got from the new
  // one at or after it.
  val cutSeparates: bool = match s.cut {
    | Waiting => true
    | At(c) => s.takenPrev.forall(g => g < c) and s.takenCur.forall(g => g >= c)
  }

  // A group the new connection delivered whole reaches the application whole
  // when the previous connection delivered it whole too, or can no longer
  // deliver it. Groups before the newest one the previous connection handed
  // out are exempt: the previous connection overtook them, and losing them
  // when it closes first is accepted (no gap fill).
  val currentWholeGroupKept: bool = GROUPS.forall(g =>
    (wholeGroup(s.curSent, g)
      and s.takenPrev.forall(p => p < g)
      and (s.previous == PEnded or wholeGroup(s.prevSent, g)))
      implies groupObjects(OBJECTS, g).subseteq(s.seen))

  // A group the previous connection delivered whole reaches the application
  // whole once the new connection has delivered everything of it that it
  // ever will.
  val previousWholeGroupKept: bool = GROUPS.forall(g =>
    (wholeGroup(s.prevSent, g)
      and groupObjects(OBJECTS, g).filter(l => not(before(l, s.first))).subseteq(s.curSent))
      implies groupObjects(OBJECTS, g).subseteq(s.seen))

  val safety: bool = and {
    noDuplicate,
    cutSeparates,
    currentWholeGroupKept,
    previousWholeGroupKept,
  }
}

// expect: violation
module group_cutover_subgroups_current {
  import group_cutover(
    GROUPS = 0.to(3), OBJECTS = 0.to(1), DATAGRAMS = false,
    CUT_FROM_START = true, KEEP_NEWEST = false,
  ).*
}

// expect: violation
module group_cutover_datagrams_current {
  import group_cutover(
    GROUPS = 0.to(3), OBJECTS = 0.to(1), DATAGRAMS = true,
    CUT_FROM_START = true, KEEP_NEWEST = false,
  ).*
}

// expect: violation
module group_cutover_subgroups_cut_from_start {
  import group_cutover(
    GROUPS = 0.to(3), OBJECTS = 0.to(1), DATAGRAMS = false,
    CUT_FROM_START = true, KEEP_NEWEST = false,
  ).*
}

// expect: ok
module group_cutover_subgroups_fixed {
  import group_cutover(
    GROUPS = 0.to(3), OBJECTS = 0.to(1), DATAGRAMS = false,
    CUT_FROM_START = true, KEEP_NEWEST = true,
  ).*
}

// expect: ok
module group_cutover_datagrams_fixed {
  import group_cutover(
    GROUPS = 0.to(3), OBJECTS = 0.to(1), DATAGRAMS = true,
    CUT_FROM_START = true, KEEP_NEWEST = true,
  ).*
}

// expect: violation
module group_cutover_subgroups_keep_newest {
  import group_cutover(
    GROUPS = 0.to(3), OBJECTS = 0.to(1), DATAGRAMS = false,
    CUT_FROM_START = false, KEEP_NEWEST = true,
  ).*
}

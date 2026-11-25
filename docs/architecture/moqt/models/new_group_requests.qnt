// A model of the NEW_GROUP_REQUEST throttle of one Origin track
// (draft-18 §10.2.13): `NewGroupRequests`
// (crates/moqt/src/modules/moqt/domains/origin/new_group_requests.rs :19-95),
// its `NewGroupDelayTask` (origin/new_group_delay_task.rs), and the Origin
// calls that drive it (origin.rs): `accept_new_group_requests` (:333-343)
// replaces the track's `NewGroupRequests`; `OriginTrack::request_new_group`
// (:252-261), reached from `accept_subscribe`, `publish` and `apply_update`,
// calls `request`; `OriginTrack::store` (:264-292), reached from every
// write, calls `object_placed`.
//
// Each action is one critical section under the Origin's `state` lock, or
// one run of the delay task, or a tick of the clock. Time counts in units
// of MIN_GROUP_DURATION / MIN_DURATION; MIN_OBJECTS stands for
// `MIN_GROUP_OBJECTS`. The application is an actor: it starts groups on its
// own (keyframes) or because it took the `requested` Notify's permit.
// broadcast calls `accept_new_group_requests` again on every VideoConfig
// (crates/broadcast/src/track_publisher.rs :134 → broadcast.rs :107-129),
// so it runs on a track already published and written.
//
// Switches:
// - ENABLE_AFTER_WRITES: `accept_new_group_requests` may come after the
//   track has an open group (its doc only asks for it before `publish` and
//   before the track is subscribed to).
// - REENABLE: it may come again on a track that has dynamic groups.
// - THROTTLE: `request` delays as the code does (off: always passes on).
// - KEEP_ON_REENABLE (fix): `accept_new_group_requests` keeps the existing
//   `NewGroupRequests` and only swaps its Notify.
// - SEED_OPEN_GROUP (fix): a new `NewGroupRequests` starts with the track's
//   open group, taken to begin now, holding the objects the numbering
//   placed in it.
//
// Commands (from docs/architecture/moqt/models, Java 21+ on PATH):
//   quint typecheck new_group_requests.qnt
//   quint run --main new_group_requests_current --invariant safety --max-samples 5000 --max-steps 30 new_group_requests.qnt
//   quint verify --backend tlc --main new_group_requests_current --invariant safety new_group_requests.qnt
//   (likewise for new_group_requests_as_documented, new_group_requests_late_enable,
//   new_group_requests_without_throttle, new_group_requests_reenable_seeded,
//   new_group_requests_fixed; `--invariant requestedGroupsFollowLongGroups` for the
//   permit carried across groups)
module new_group_requests {
  const MIN_DURATION: int
  const MIN_OBJECTS: int
  const MAX_TIME: int
  const ENABLE_AFTER_WRITES: bool
  const REENABLE: bool
  const THROTTLE: bool
  const KEEP_ON_REENABLE: bool
  const SEED_OPEN_GROUP: bool

  type Group = { began: int, objects: int }

  // `Option<OpenGroup>`, and the track's actual open group.
  type OpenGroup = NoGroup | Open(Group)

  // `Option<Outstanding>`, with the delay task's progress.
  type Outstanding =
    | NotOutstanding
    | PassedOn
    | Delayed(int)  // task sleeping until this time
    | DelayEnded    // `end_delay` called; the task has not notified yet
    | DelayDone     // the task notified; still `Outstanding::Delayed`

  // `OriginTrack::new_group_requests`
  type Requests =
    | Disabled
    | Enabled({ openGroup: OpenGroup, outstanding: Outstanding })

  var now: int
  var track: OpenGroup
  var requests: Requests
  // The `requested` Notify's stored permit.
  var permit: bool
  // Observers: notifications in the track's open group, one sent while it
  // was short, a request accepted and not notified, and a group the
  // application started on a permit while the open one was short.
  var notifies: int
  var earlyNotify: bool
  var pending: bool
  var earlyRequestedGroup: bool

  pure def short(g: OpenGroup, t: int): bool = match g {
    | NoGroup => false
    | Open(o) => t - o.began < MIN_DURATION and o.objects < MIN_OBJECTS
  }

  // `OpenGroup::delay`, as the time the request is passed on.
  pure def deadline(g: OpenGroup, t: int): int = match g {
    | NoGroup => t
    | Open(o) =>
      if (not(THROTTLE) or o.objects >= MIN_OBJECTS) t
      else if (o.began + MIN_DURATION > t) o.began + MIN_DURATION
      else t
  }

  pure def isDelayed(o: Outstanding): bool = match o {
    | Delayed(_) => true
    | DelayEnded => true
    | _ => false
  }

  pure def delayedUntil(o: Outstanding): int = match o {
    | Delayed(d) => d
    | _ => 0
  }

  def isEnabled: bool = requests != Disabled

  def outstanding: Outstanding = match requests {
    | Disabled => NotOutstanding
    | Enabled(r) => r.outstanding
  }

  def ngrOpenGroup: OpenGroup = match requests {
    | Disabled => NoGroup
    | Enabled(r) => r.openGroup
  }

  // `requested.notify_one()`
  action notifyApp(after: Outstanding): bool = all {
    requests' = Enabled({ openGroup: ngrOpenGroup, outstanding: after }),
    permit' = true,
    notifies' = if (notifies < 2) notifies + 1 else notifies,
    earlyNotify' = (earlyNotify or short(track, now)),
    pending' = false,
  }

  action init = all {
    now' = 0,
    track' = NoGroup,
    requests' = Disabled,
    permit' = false,
    notifies' = 0,
    earlyNotify' = false,
    pending' = false,
    earlyRequestedGroup' = false,
  }

  action tick = all {
    now < MAX_TIME,
    now' = now + 1,
    track' = track, requests' = requests, permit' = permit,
    notifies' = notifies, earlyNotify' = earlyNotify, pending' = pending,
    earlyRequestedGroup' = earlyRequestedGroup,
  }

  // `accept_new_group_requests`
  action enable = all {
    isEnabled implies REENABLE,
    track != NoGroup implies ENABLE_AFTER_WRITES,
    requests' =
      if (isEnabled and KEEP_ON_REENABLE) requests
      else Enabled({
        openGroup: match track {
          | NoGroup => NoGroup
          | Open(o) => if (SEED_OPEN_GROUP) Open({ began: now, objects: o.objects }) else NoGroup
        },
        outstanding: NotOutstanding,
      }),
    // A replaced request's delay task is dropped (aborted) unnotified; the
    // request stays accepted.
    pending' = pending,
    now' = now, track' = track, permit' = permit,
    notifies' = notifies, earlyNotify' = earlyNotify,
    earlyRequestedGroup' = earlyRequestedGroup,
  }

  // `OriginTrack::request_new_group` for a group after the open one →
  // `NewGroupRequests::request`.
  action request = all {
    isEnabled,
    now' = now, track' = track, earlyRequestedGroup' = earlyRequestedGroup,
    if (outstanding != NotOutstanding) all {
      requests' = requests, permit' = permit, notifies' = notifies,
      earlyNotify' = earlyNotify, pending' = pending,
    } else if (deadline(ngrOpenGroup, now) == now) notifyApp(PassedOn)
    else all {
      requests' = Enabled({
        openGroup: ngrOpenGroup,
        outstanding: Delayed(deadline(ngrOpenGroup, now)),
      }),
      permit' = permit, notifies' = notifies, earlyNotify' = earlyNotify,
      pending' = true,
    },
  }

  // The delay task wakes: its sleep ended, or `end_delay` was called.
  action delayTaskNotifies = all {
    isDelayed(outstanding),
    outstanding == DelayEnded or delayedUntil(outstanding) <= now,
    now' = now, track' = track, earlyRequestedGroup' = earlyRequestedGroup,
    notifyApp(DelayDone),
  }

  // `NewGroupRequests::object_placed` for an object that starts a group.
  def startedRequests(t: int): Requests = match requests {
    | Disabled => Disabled
    | Enabled(_) => Enabled({ openGroup: Open({ began: t, objects: 1 }), outstanding: NotOutstanding })
  }

  // A write that starts a group. `onPermit`: the application took the
  // Notify's permit and starts the group the request asked for.
  action startGroup(onPermit: bool): bool = all {
    onPermit implies permit,
    now' = now,
    track' = Open({ began: now, objects: 1 }),
    requests' = startedRequests(now),
    permit' = if (onPermit) false else permit,
    notifies' = 0,
    earlyNotify' = earlyNotify,
    pending' = false,
    earlyRequestedGroup' = (earlyRequestedGroup or (onPermit and short(track, now))),
  }

  pure def grown(g: OpenGroup): OpenGroup = match g {
    | NoGroup => NoGroup
    | Open(o) => Open({ ...o, objects: if (o.objects < MIN_OBJECTS) o.objects + 1 else o.objects })
  }

  // A write within the open group: `object_placed` counts it and ends the
  // delay once the group holds MIN_OBJECTS.
  action writeWithin = all {
    track != NoGroup,
    now' = now,
    track' = grown(track),
    requests' = match requests {
      | Disabled => Disabled
      | Enabled(r) => {
        val g = grown(r.openGroup)
        val long = match g {
          | NoGroup => false
          | Open(o) => o.objects >= MIN_OBJECTS
        }
        Enabled({
          openGroup: g,
          outstanding: match r.outstanding {
            | Delayed(_) => if (long) DelayEnded else r.outstanding
            | _ => r.outstanding
          },
        })
      }
    },
    permit' = permit, notifies' = notifies, earlyNotify' = earlyNotify,
    pending' = pending, earlyRequestedGroup' = earlyRequestedGroup,
  }

  action step = any {
    tick,
    enable,
    request,
    delayTaskNotifies,
    startGroup(true),
    startGroup(false),
    writeWithin,
  }

  // The application is never asked for a new group while the open one is
  // younger than MIN_DURATION and holds fewer than MIN_OBJECTS objects.
  val notifiedOnlyWhenLongEnough: bool = not(earlyNotify)

  // A request is passed on at most once per open group.
  val oneNotifyPerGroup: bool = notifies <= 1

  // A request accepted for the open group is passed on or still scheduled:
  // the subscriber's relay keeps it outstanding until the group changes and
  // sends no other (draft-18 §10.2.13).
  val requestNotLost: bool = pending implies isDelayed(outstanding)

  val safety: bool = and {
    notifiedOnlyWhenLongEnough,
    oneNotifyPerGroup,
    requestNotLost,
  }

  // Not in `safety`: a permit the application takes after starting a group
  // of its own starts a group right after that one. The Notify keeps the
  // permit across groups; `object_placed` cannot take it back.
  val requestedGroupsFollowLongGroups: bool = not(earlyRequestedGroup)
}

// expect: violation
module new_group_requests_current {
  import new_group_requests(
    MIN_DURATION = 2, MIN_OBJECTS = 2, MAX_TIME = 4,
    ENABLE_AFTER_WRITES = true, REENABLE = true, THROTTLE = true,
    KEEP_ON_REENABLE = false, SEED_OPEN_GROUP = false,
  ).*
}

// Enabled once, before the first write: the code passes.
// expect: ok
module new_group_requests_as_documented {
  import new_group_requests(
    MIN_DURATION = 2, MIN_OBJECTS = 2, MAX_TIME = 4,
    ENABLE_AFTER_WRITES = false, REENABLE = false, THROTTLE = true,
    KEEP_ON_REENABLE = false, SEED_OPEN_GROUP = false,
  ).*
}

// expect: violation
module new_group_requests_late_enable {
  import new_group_requests(
    MIN_DURATION = 2, MIN_OBJECTS = 2, MAX_TIME = 4,
    ENABLE_AFTER_WRITES = true, REENABLE = false, THROTTLE = true,
    KEEP_ON_REENABLE = false, SEED_OPEN_GROUP = false,
  ).*
}

// expect: violation
module new_group_requests_without_throttle {
  import new_group_requests(
    MIN_DURATION = 2, MIN_OBJECTS = 2, MAX_TIME = 4,
    ENABLE_AFTER_WRITES = false, REENABLE = false, THROTTLE = false,
    KEEP_ON_REENABLE = false, SEED_OPEN_GROUP = false,
  ).*
}

// expect: ok
module new_group_requests_fixed {
  import new_group_requests(
    MIN_DURATION = 2, MIN_OBJECTS = 2, MAX_TIME = 4,
    ENABLE_AFTER_WRITES = true, REENABLE = true, THROTTLE = true,
    KEEP_ON_REENABLE = true, SEED_OPEN_GROUP = true,
  ).*
}

// Re-enabling with the late-enable half of the fix only: what replacing the
// `NewGroupRequests` alone does.
// expect: violation
module new_group_requests_reenable_seeded {
  import new_group_requests(
    MIN_DURATION = 2, MIN_OBJECTS = 2, MAX_TIME = 4,
    ENABLE_AFTER_WRITES = true, REENABLE = true, THROTTLE = true,
    KEEP_ON_REENABLE = false, SEED_OPEN_GROUP = true,
  ).*
}

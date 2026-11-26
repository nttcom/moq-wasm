// A model of the client follow/watch layer (crates/moqt/src/modules/client):
// - `Follow::run` and `PublisherHandover` (track_follower.rs): the follow
//   task's loop. One action is one turn of the loop: one `Step` handled up to
//   the next `select!`, except that `self.subscribe().await` is split into
//   sending the SUBSCRIBE and taking its answer, since announcements queue
//   meanwhile.
// - `Routes::route` / `Routes::end` (routes.rs): the session's event loop and
//   what it closes when the session ends; `NamespaceWatchers::close`
//   (moqt/domains/namespace_watchers.rs) ends every announcement receiver.
// - `TrackWatch` (track_watch.rs): `WatchTaker`, the handler in
//   `Routes::handlers` that owns the sender of the watch's events, and
//   `TrackWatch::next`, which returns `None` only when `close` was called or
//   that sender is dropped.
//
// The peer (a relay) reports the namespace as one aggregate: PUBLISH_NAMESPACE
// when its first publisher announces it, PUBLISH_NAMESPACE_DONE when its last
// one withdraws it (services/relay: control_message_forwarder.rs
// `publish_namespace` sends one PUBLISH_NAMESPACE per subscriber session;
// directory.rs withdraws once no client publisher remains). The followed
// subscription may end (PUBLISH_DONE) at any time.
//
// Switches:
// - CLEAR_ON_WITHDRAWAL: a withdrawal of the namespace (`active == false`)
//   clears `PublisherHandover::announced_while_following`.
// - END_DROPS_HANDLERS: `Routes::end` also drops the handlers, so every
//   `WatchTaker` / `PublishTaker` sender goes.
// - END_CLOSES_WATCHERS: `Routes::end` calls `NamespaceWatchers::close`
//   (true in the code; `track_follower_without_watcher_close` turns it off).
//
// Check (from this directory, Java 21+ on PATH; `quint` is
// @informalsystems/quint 0.34):
//   quint typecheck track_follower.qnt
//   quint verify --backend tlc --main track_follower_current --invariant safety track_follower.qnt
//   quint verify --backend tlc --main track_follower_fixed --invariant safety track_follower.qnt
// and likewise for the other instances at the end of the file.
module track_follower {
  const CLEAR_ON_WITHDRAWAL: bool
  const END_DROPS_HANDLERS: bool
  const END_CLOSES_WATCHERS: bool

  pure val PUBLISHERS = Set("p1", "p2")
  // Announcements queued for the follow task.
  pure val MAX_INBOX = 2

  // The follow task (`followed` and `handover` in `Follow::run`).
  type Follower =
    | Idle                  // `followed == None`
    | Subscribing           // in `self.subscribe().await`
    | Following             // `followed == Some`, no handover
    | FollowingWithHandover // `followed == Some`, `announced_while_following`
    | Ended                 // `run` returned: `TrackFollower::next` returns None

  type Phase =
    | Live
    | SessionEnded // the transport closed; `Routes::route` has not ended yet
    | RoutesEnded  // `Routes::end` ran

  // The application's `TrackWatch::next`.
  type WatchNext = NotWaiting | Waiting | ReturnedNone

  type State = {
    follower: Follower,
    // NamespaceEvent.active of the announcements queued for the follow task.
    inbox: List[bool],
    // The publishers that announced the namespace at the relay.
    announcers: Set[str],
    // The last announcement the follow task took (false before any).
    lastSeen: bool,
    // The follow task sent a SUBSCRIBE while the last announcement it took
    // was a withdrawal.
    subscribedWithdrawn: bool,
    phase: Phase,
    // The follow task's announcement sender is in `NamespaceWatchers`.
    announcementsOpen: bool,
    // `PublishTaker` / `WatchTaker` are in `Routes::handlers`.
    handlersHeld: bool,
    watchClosed: bool,
    watchNext: WatchNext,
  }

  var s: State

  action init = s' = {
    follower: Idle,
    inbox: List(),
    announcers: Set(),
    lastSeen: false,
    subscribedWithdrawn: false,
    phase: Live,
    announcementsOpen: true,
    handlersHeld: true,
    watchClosed: false,
    watchNext: NotWaiting,
  }

  def following: bool = Set(Following, FollowingWithHandover).contains(s.follower)

  // A publisher announces the namespace; the relay forwards the first one.
  action announce(p: str): bool = all {
    s.phase == Live,
    not(s.announcers.contains(p)),
    s.announcers.size() > 0 or s.inbox.length() < MAX_INBOX,
    s' = {
      ...s,
      announcers: s.announcers.union(Set(p)),
      inbox: if (s.announcers.size() == 0) s.inbox.append(true) else s.inbox,
    },
  }

  // A publisher withdraws the namespace; the relay forwards the last one.
  action withdraw(p: str): bool = all {
    s.phase == Live,
    s.announcers.contains(p),
    s.announcers.size() > 1 or s.inbox.length() < MAX_INBOX,
    s' = {
      ...s,
      announcers: s.announcers.exclude(Set(p)),
      inbox: if (s.announcers.size() == 1) s.inbox.append(false) else s.inbox,
    },
  }

  // `Step::Announced`.
  action takeAnnouncement = all {
    s.inbox.length() > 0,
    s.follower != Subscribing,
    s.follower != Ended,
    val active = s.inbox.head()
    val taken = { ...s, inbox: s.inbox.tail(), lastSeen: active }
    s' = if (active and s.follower == Idle)
      { ...taken, follower: Subscribing }
    else if (active)
      { ...taken, follower: FollowingWithHandover }
    else if (CLEAR_ON_WITHDRAWAL and s.follower == FollowingWithHandover)
      { ...taken, follower: Following }
    else taken,
  }

  // The answer to the follower's SUBSCRIBE: SUBSCRIBE_OK needs an announcer
  // and a live session; any request may fail.
  action subscribeAnswered(ok: bool): bool = all {
    s.follower == Subscribing,
    not(ok) or (s.phase == Live and s.announcers.size() > 0),
    s' = { ...s, follower: if (ok) Following else Idle },
  }

  // `Step::Published` while nothing is followed: PUBLISH_OK.
  action takePublish = all {
    s.phase == Live,
    s.handlersHeld,
    s.follower == Idle,
    s' = { ...s, follower: Following },
  }

  // `Step::Left`: the followed subscription ended; `followed_publisher_left`
  // decides whether to subscribe again.
  action followedLeft = all {
    following,
    s' = if (s.follower == FollowingWithHandover) {
      ...s,
      follower: Subscribing,
      subscribedWithdrawn: s.subscribedWithdrawn or not(s.lastSeen),
    } else { ...s, follower: Idle },
  }

  // `Step::Ended`: a `recv` of the announcements or of the PUBLISH handlers
  // returned None.
  action followerEnds = all {
    s.follower != Subscribing,
    s.follower != Ended,
    (not(s.announcementsOpen) and s.inbox.length() == 0) or not(s.handlersHeld),
    s' = { ...s, follower: Ended },
  }

  action sessionEnds = all {
    s.phase == Live,
    s' = { ...s, phase: SessionEnded },
  }

  // `Routes::route` returns and calls `Routes::end`.
  action routesEnd = all {
    s.phase == SessionEnded,
    s' = {
      ...s,
      phase: RoutesEnded,
      announcementsOpen: s.announcementsOpen and not(END_CLOSES_WATCHERS),
      handlersHeld: s.handlersHeld and not(END_DROPS_HANDLERS),
    },
  }

  action watchCallsNext = all {
    s.watchNext == NotWaiting,
    s' = { ...s, watchNext: Waiting },
  }

  // `TrackWatch::next` returns None: closed, or the event sender dropped.
  action watchNextEnds = all {
    s.watchNext == Waiting,
    s.watchClosed or not(s.handlersHeld),
    s' = { ...s, watchNext: ReturnedNone },
  }

  action watchClose = all {
    not(s.watchClosed),
    s' = { ...s, watchClosed: true },
  }

  action step = any {
    nondet p = oneOf(PUBLISHERS)
    any { announce(p), withdraw(p) },
    takeAnnouncement,
    subscribeAnswered(true),
    subscribeAnswered(false),
    takePublish,
    followedLeft,
    followerEnds,
    sessionEnds,
    routesEnd,
    watchCallsNext,
    watchNextEnds,
    watchClose,
  }

  // The follower sends SUBSCRIBE only for a namespace whose last
  // announcement it took was PUBLISH_NAMESPACE, not PUBLISH_NAMESPACE_DONE.
  val subscribesOnlyAnnounced: bool = not(s.subscribedWithdrawn)

  // Once the session's routes have ended, a waiting `TrackWatch::next`
  // returns.
  val everyWatchEnds: bool =
    (s.phase == RoutesEnded and s.watchNext == Waiting)
      implies (s.watchClosed or not(s.handlersHeld))

  // Once the session's routes have ended and the follow task is between
  // steps with nothing queued, it ends, so `TrackFollower::next` returns None.
  val everyFollowerEnds: bool =
    (s.phase == RoutesEnded and s.inbox.length() == 0
      and Set(Idle, Following, FollowingWithHandover).contains(s.follower))
      implies (not(s.announcementsOpen) or not(s.handlersHeld))

  val safety: bool = and {
    subscribesOnlyAnnounced,
    everyWatchEnds,
    everyFollowerEnds,
  }
}

// expect: violation
module track_follower_current {
  import track_follower(
    CLEAR_ON_WITHDRAWAL = false,
    END_DROPS_HANDLERS = false,
    END_CLOSES_WATCHERS = true,
  ).*
}

// expect: ok
module track_follower_fixed {
  import track_follower(
    CLEAR_ON_WITHDRAWAL = true,
    END_DROPS_HANDLERS = true,
    END_CLOSES_WATCHERS = true,
  ).*
}

// Each fix but one: shows the remaining bug on its own.
// expect: violation
module track_follower_without_clear_fix {
  import track_follower(
    CLEAR_ON_WITHDRAWAL = false,
    END_DROPS_HANDLERS = true,
    END_CLOSES_WATCHERS = true,
  ).*
}

// expect: violation
module track_follower_without_end_fix {
  import track_follower(
    CLEAR_ON_WITHDRAWAL = true,
    END_DROPS_HANDLERS = false,
    END_CLOSES_WATCHERS = true,
  ).*
}

// The follower relies on `Routes::end` closing the namespace watchers: without
// it (and with the handlers kept, as today) it never ends.
// expect: violation
module track_follower_without_watcher_close {
  import track_follower(
    CLEAR_ON_WITHDRAWAL = true,
    END_DROPS_HANDLERS = false,
    END_CLOSES_WATCHERS = false,
  ).*
}

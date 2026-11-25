// A model of requests this side sends on a draft-18 session, each on its
// own bidirectional stream (docs/spec/draft-ietf-moq-transport-18.txt §3.3,
// §3.3.2, §5.1, §10.1).
//
// Part A, Request IDs of PUBLISH_NAMESPACE
// (crates/moqt/src/modules/moqt/version/draft18.rs `send_request`,
// `advertise_namespace`, `consume`; domains/publisher.rs `announce` and
// `send_publish_namespace`; domains/session_context.rs
// `published_namespaces`, keyed by the logical Request ID):
// - `announceSend(ns, local)`: `send_publish_namespace` under the Request
//   ID reservation. When the peer has a SUBSCRIBE_NAMESPACE covering `ns`
//   (`local`), `advertise_namespace` answers PUBLISH_NAMESPACE_OK itself and
//   `send_request` returns, consuming the ID only with CONSUME_LOCAL;
//   otherwise the request is written and the ID consumed. The reservation
//   serializes these, so one action is one whole `send_request`.
// - `announceRecord(ns)`: `announce` inserts `published_namespaces[id] = ns`
//   after `send_publish_namespace` returned (another announce may reserve
//   in between).
//
// Part B, one SUBSCRIBE from `send_request` on (draft18.rs `send_request`,
// `PendingResponse`, `RequestCancel(CallerState)`, `ReceivedResponse`,
// `end_outgoing`, `take_outgoing`; draft18/request_streams.rs
// `OutgoingRequestTask::read`, `FirstMessage`, `abandoned`, `peer_ended`;
// domains/subscriber.rs `subscribe_ok` / `register_data_receiver` /
// `accept`; domains/connection.rs `ObjectSinks`).
// The request is written and `outgoing[id]` inserted before the first
// action. Actions:
// - `peerRespond`: the publisher answers SUBSCRIBE_OK, REQUEST_ERROR, a
//   FIN or reset without a response, or bytes that do not decode.
// - `peerObject`: an object on the SUBSCRIBE_OK's Track Alias reaches
//   `ObjectSinks` (a Buffer when no receiver is registered).
// - `taskFirst`: `OutgoingRequestTask::read` up to its first message. Its
//   `receive_unless_cancelled` races the response against `reader_stop`
//   (notified by this side's cancel); tokio::select! may take either when
//   both are ready (`FirstMessage::Cancelled` / `Received` / `Finished` /
//   `Closed` / `Malformed`). A SUBSCRIBE_OK whose `response.send` fails
//   (the caller dropped `PendingResponse`) runs `abandoned`
//   (`cancel_unclaimed_track_alias`).
//   A reset removes the entry and FINs the stream (`end_unanswered`); with
//   FIX_FIRST_MESSAGE it then answers the caller with a REQUEST_ERROR
//   (§3.3.2: the peer cancelled the request), otherwise drops the response
//   sender. A FIN or decode error does the same as a reset without the fix;
//   with it, `close_on_violation` closes the session (§5.1).
// - `callerReceive`: `PendingResponse::response` takes the oneshot
//   (`RequestCancel::deliver`): a success response moves `CallerState` to
//   `Delivered`; an error response resets the stream from
//   `AwaitingResponse`, as does a dropped sender.
// - `callerDrop`: the caller drops `PendingResponse` (or times out), or
//   drops `ReceivedResponse` before `accept`, or its future inside
//   `register_data_receiver` (between `install_receiver` and the
//   `receiver_map` insert). `RequestCancel` dropped in `AwaitingResponse`
//   or `Delivered` runs `end_outgoing`: takes the entry and resets the
//   stream, notifying `reader_stop`. With
//   CLEAN_ABANDONED_ALIAS it also does what draft-14's `handle_late_response`
//   does: `cancel_unclaimed_track_alias` and `cancel_subscription`
//   (`abandon_track_alias`, for a SUBSCRIBE_OK `Delivered` or left in the
//   oneshot when `PendingResponse` is dropped).
// - `callerInstall` / `callerAccept`: `register_data_receiver`
//   (`install_receiver`, then the `receiver_map` lock), then `accept`
//   (`CallerState::Ended`).
// - `appUnsubscribe` / `peerEnds`: `notify(Unsubscribe)` (take + reset, and
//   the subscription's sink is cancelled) and `peer_ended` after
//   SUBSCRIBE_OK (take + FIN): the other removers of the entry.
// With TAKE_ATOMIC off, a remover checks the entry under one lock hold and
// removes it under another (`removerEnd`).
//
// `draft18_outgoing_request_current` is the implementation, with every fix
// switch on, and passes, as does `draft18_outgoing_request_fixed`.
// `draft18_outgoing_request_take_split` turns TAKE_ATOMIC off and fails
// `streamEndedOnce`.
//
// Check (from docs/architecture/moqt/models, Java 21+):
//   quint typecheck draft18_outgoing_request.qnt
//   quint verify --backend tlc --main draft18_outgoing_request_current --invariant safety draft18_outgoing_request.qnt   # passes
//   quint verify --backend tlc --main draft18_outgoing_request_fixed --invariant safety draft18_outgoing_request.qnt     # passes
//   quint verify --backend tlc --main draft18_outgoing_request_take_split --invariant safety draft18_outgoing_request.qnt
module draft18_outgoing_request {
  const NAMESPACES: Set[str]
  const CONSUME_LOCAL: bool
  const CLEAN_ABANDONED_ALIAS: bool
  const FIX_FIRST_MESSAGE: bool
  const TAKE_ATOMIC: bool

  // ---- Part A ----

  // One PUBLISH_NAMESPACE as `announce` sees it.
  type Announce =
    | NIdle
    | NSent(int)   // `send_publish_namespace` returned with this Request ID
    | NRecorded(int)

  // ---- Part B ----

  // What the publisher sent first on the SUBSCRIBE's stream.
  type Peer = PIdle | POk | PError | PFinNoResponse | PResetNoResponse | PGarbage

  // What `OutgoingRequestTask::read` did with the first message.
  type First =
    | Unread
    | ReadOk
    | ReadError
    | ReadFin
    | ReadReset
    | ReadGarbage
    | StoppedFirst  // `reader_stop` won the select; nothing was read

  // The oneshot from the task to `PendingResponse`.
  type Slot = SlotEmpty | SlotOk | SlotError | SlotTaken | SlotSenderGone

  type Caller =
    | CWaiting     // holds `PendingResponse`
    | CGot         // holds `ReceivedResponse`, not accepted
    | CInstalling  // inside `register_data_receiver`: sink installed, `receiver_map` not yet
    | CAccepted
    | CDropped     // dropped one of the above, or timed out
    | CDone        // got a failure result
    | CUnsubscribed

  type Outcome = ONone | OOk | ORejected | OSessionClosed

  // `ObjectSinks` entry of the SUBSCRIBE_OK's Track Alias.
  type Sink = SinkNone | SinkBuffer | SinkReceiver | SinkCancelled

  type Half = HOpen | HFin | HReset

  type State = {
    nextId: int,
    announces: str -> Announce,
    published: int -> str,
    entry: bool,
    half: Half,
    ends: int,
    // Removers that saw the entry and will end the stream (TAKE_ATOMIC off).
    pendingEnds: Set[str],
    readerStopped: bool,
    peer: Peer,
    first: First,
    slot: Slot,
    caller: Caller,
    outcome: Outcome,
    sink: Sink,
    violationClose: bool,
  }

  var s: State

  action init = s' = {
    nextId: 0,
    announces: NAMESPACES.mapBy(_ => NIdle),
    published: Map(),
    entry: true,
    half: HOpen,
    ends: 0,
    pendingEnds: Set(),
    readerStopped: false,
    peer: PIdle,
    first: Unread,
    slot: SlotEmpty,
    caller: CWaiting,
    outcome: ONone,
    sink: SinkNone,
    violationClose: false,
  }

  // ---- Part A actions ----

  action announceSend(ns: str, local: bool): bool = all {
    s.announces.get(ns) == NIdle,
    s' = {
      ...s,
      announces: s.announces.set(ns, NSent(s.nextId)),
      nextId: if (local and not(CONSUME_LOCAL)) s.nextId else s.nextId + 1,
    },
  }

  pure def sentId(a: Announce): int = match a { | NSent(i) => i | _ => -1 }

  action announceRecord(ns: str): bool = all {
    sentId(s.announces.get(ns)) >= 0,
    val id = sentId(s.announces.get(ns))
    s' = {
      ...s,
      announces: s.announces.set(ns, NRecorded(id)),
      published: s.published.put(id, ns),
    },
  }

  // ---- Part B actions ----

  // A remover of `outgoing[id]` ending this side's half with `how`.
  def removed(st: State, who: str, how: Half): State =
    if (not(st.entry) or st.pendingEnds.contains(who)) st
    else if (TAKE_ATOMIC)
      { ...st, entry: false, ends: st.ends + 1, half: how,
        readerStopped: st.readerStopped or how == HReset }
    else
      { ...st, pendingEnds: st.pendingEnds.union(Set(who)) }

  action removerEnd(who: str): bool = all {
    s.pendingEnds.contains(who),
    s' = {
      ...s,
      pendingEnds: s.pendingEnds.exclude(Set(who)),
      entry: false,
      ends: s.ends + 1,
      half: if (who == "first" or who == "peerEnd") HFin else HReset,
      readerStopped: s.readerStopped or not(who == "first" or who == "peerEnd"),
    },
  }

  action peerRespond: bool = all {
    s.peer == PIdle,
    nondet p = oneOf(Set(POk, PError, PFinNoResponse, PResetNoResponse, PGarbage))
    s' = { ...s, peer: p },
  }

  action peerObject: bool = all {
    s.peer == POk,
    s.sink == SinkNone,
    s' = { ...s, sink: SinkBuffer },
  }

  def callerAlive: bool = s.caller == CWaiting

  // `cancel_unclaimed`: cancels the alias unless a receiver is registered.
  def unclaimedCancelled(sink: Sink): Sink =
    if (sink == SinkReceiver) sink else SinkCancelled

  // `end_unanswered` for a FIN or decode error before FIX_FIRST_MESSAGE.
  def unansweredEnd(st: State): State =
    if (FIX_FIRST_MESSAGE) st else removed(st, "first", HFin)

  action taskFirst: bool = all {
    s.first == Unread,
    s.peer != PIdle or s.readerStopped,
    any {
      // `reader_stop` taken.
      all {
        s.readerStopped,
        s' = { ...s, first: StoppedFirst,
               slot: if (s.slot == SlotEmpty) SlotSenderGone else s.slot },
      },
      // The first message taken.
      all {
        s.peer != PIdle,
        if (s.peer == POk)
            if (callerAlive) s' = { ...s, first: ReadOk, slot: SlotOk }
            // `abandoned`
            else s' = { ...s, first: ReadOk, sink: unclaimedCancelled(s.sink) }
        else if (s.peer == PError)
            s' = { ...s, first: ReadError, slot: if (callerAlive) SlotError else s.slot }
        else if (s.peer == PResetNoResponse)
            if (FIX_FIRST_MESSAGE)
              // The peer cancelled the request (§3.3.2): a rejection.
              s' = { ...removed(s, "first", HFin), first: ReadReset,
                     slot: if (callerAlive) SlotError else s.slot }
            else
              s' = { ...removed(s, "first", HFin), first: ReadReset,
                     slot: if (callerAlive) SlotSenderGone else s.slot }
        // With FIX_FIRST_MESSAGE the session close ends the stream.
        else if (s.peer == PFinNoResponse)
            s' = { ...unansweredEnd(s), first: ReadFin,
                   slot: if (callerAlive) SlotSenderGone else s.slot,
                   violationClose: s.violationClose or FIX_FIRST_MESSAGE }
        else
            s' = { ...unansweredEnd(s), first: ReadGarbage,
                   slot: if (callerAlive) SlotSenderGone else s.slot,
                   violationClose: s.violationClose or FIX_FIRST_MESSAGE },
      },
    },
  }

  action callerReceive: bool = all {
    s.caller == CWaiting,
    s.slot != SlotEmpty,
    s.slot != SlotTaken,
    if (s.slot == SlotOk) s' = { ...s, slot: SlotTaken, caller: CGot }
    // An error response leaves `RequestCancel` armed.
    else if (s.slot == SlotError)
      s' = { ...removed(s, "cancel", HReset), slot: SlotTaken, caller: CDone, outcome: ORejected }
    else
      s' = { ...removed(s, "cancel", HReset), slot: SlotTaken, caller: CDone, outcome: OSessionClosed },
  }

  // The alias this side knows of: the task read SUBSCRIBE_OK.
  def aliasKnown(st: State): bool = st.first == ReadOk

  action callerDrop: bool = all {
    s.caller == CWaiting or s.caller == CGot or s.caller == CInstalling,
    val r = removed(s, "cancel", HReset)
    s' = {
      ...r,
      caller: CDropped,
      sink: if (CLEAN_ABANDONED_ALIAS and aliasKnown(s)) SinkCancelled else s.sink,
    },
  }

  action callerInstall: bool = all {
    s.caller == CGot,
    s.sink != SinkReceiver,
    s' = { ...s, caller: CInstalling, sink: SinkReceiver },
  }

  action callerAccept: bool = all {
    s.caller == CInstalling,
    s' = { ...s, caller: CAccepted, outcome: OOk },
  }

  action appUnsubscribe: bool = all {
    s.caller == CAccepted,
    s' = { ...removed(s, "unsub", HReset), caller: CUnsubscribed, sink: SinkCancelled },
  }

  action peerEnds: bool = all {
    s.first == ReadOk,
    s' = removed(s, "peerEnd", HFin),
  }

  action step = any {
    nondet ns = oneOf(NAMESPACES)
    nondet local = oneOf(Set(true, false))
    any { announceSend(ns, local), announceRecord(ns) },
    peerRespond,
    peerObject,
    taskFirst,
    callerReceive,
    callerDrop,
    callerInstall,
    callerAccept,
    appUnsubscribe,
    peerEnds,
    nondet who = oneOf(Set("first", "cancel", "unsub", "peerEnd"))
    removerEnd(who),
  }

  // §10.1 / the session's records: no two PUBLISH_NAMESPACEs this side made
  // hold the same Request ID.
  val requestIdsDistinct: bool =
    NAMESPACES.forall(a => NAMESPACES.forall(b =>
      a == b or match s.announces.get(a) {
        | NIdle => true
        | NSent(i) => s.announces.get(b) != NSent(i) and s.announces.get(b) != NRecorded(i)
        | NRecorded(i) => s.announces.get(b) != NSent(i) and s.announces.get(b) != NRecorded(i)
      }))

  // Every namespace `announce` recorded is still in `published_namespaces`
  // (what `publishes_namespace` answers and a migration announces again).
  val everyAnnouncedNamespaceRecorded: bool =
    NAMESPACES.forall(ns => match s.announces.get(ns) {
      | NRecorded(id) => s.published.keys().contains(id) and s.published.get(id) == ns
      | _ => true
    })

  // Whoever removes the entry ends the stream, exactly once.
  val streamEndedOnce: bool = s.ends <= 1

  val removedEntryEndedStream: bool =
    (not(s.entry) and s.pendingEnds == Set()) implies s.ends == 1

  // A SUBSCRIBE_OK whose caller gave up has its Track Alias cancelled once
  // this side has read it: later objects are discarded (§11.1), no stale
  // Buffer is handed to the next subscription on that alias, and no orphan
  // receiver makes the peer's later reuse of the alias a
  // DUPLICATE_TRACK_ALIAS close.
  val abandonedAliasCancelled: bool =
    (s.caller == CDropped and aliasKnown(s)) implies s.sink == SinkCancelled

  // The connection is never lost here: a request fails as `SessionClosed`
  // only when this side closes the session. One the peer refuses or cancels
  // does not (a migrating session waits a `SessionClosed` out in
  // `moved_after_loss`).
  val noSessionClosedWhileLive: bool = s.outcome == OSessionClosed implies s.violationClose

  // A first message that does not decode, or a FIN without the response
  // §5.1 requires, closes the session as a protocol violation.
  val malformedResponseClosesSession: bool =
    (s.first == ReadGarbage or s.first == ReadFin) implies s.violationClose

  val safety: bool = and {
    requestIdsDistinct,
    everyAnnouncedNamespaceRecorded,
    streamEndedOnce,
    removedEntryEndedStream,
    abandonedAliasCancelled,
    noSessionClosedWhileLive,
    malformedResponseClosesSession,
  }
}

// expect: ok
module draft18_outgoing_request_current {
  import draft18_outgoing_request(
    NAMESPACES = Set("a", "b"),
    CONSUME_LOCAL = true,
    CLEAN_ABANDONED_ALIAS = true,
    FIX_FIRST_MESSAGE = true,
    TAKE_ATOMIC = true,
  ).*
}

// expect: ok
module draft18_outgoing_request_fixed {
  import draft18_outgoing_request(
    NAMESPACES = Set("a", "b"),
    CONSUME_LOCAL = true,
    CLEAN_ABANDONED_ALIAS = true,
    FIX_FIRST_MESSAGE = true,
    TAKE_ATOMIC = true,
  ).*
}

// expect: violation
module draft18_outgoing_request_take_split {
  import draft18_outgoing_request(
    NAMESPACES = Set("a", "b"),
    CONSUME_LOCAL = true,
    CLEAN_ABANDONED_ALIAS = true,
    FIX_FIRST_MESSAGE = true,
    TAKE_ATOMIC = false,
  ).*
}

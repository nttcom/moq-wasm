// A model of one FETCH the peer sent on a draft-18 session, as the publisher
// (this side) serves it (docs/spec/draft-ietf-moq-transport-18.txt §5.2).
//
// Code (crates/moqt/src/modules/moqt/):
// - version/draft18.rs: the `Requests::incoming` entry with `fetch`
//   (`ServedFetch`: `Open`, `ResponseAttached`, `OkSent`, `ResponseEnded`;
//   the entry's removal is `Done`); `advance_fetch`,
//   `fetch_response_ended`, `attach_fetch_response`, `reset_cancelled_fetch`,
//   `send_on_incoming`, `respond(FetchOk)` and `respond(Error)`.
// - version/draft18/request_streams.rs: `IncomingRequestTask::serve` watching
//   `sender.stopped()` and `peer_ended`, both ending in `fetch_cancelled`.
// - data_plane/stream/fetch_data_sender.rs: `FetchDataSender::new`, `close`,
//   `reset`, `send_object`'s `stopped_by_peer`, and its `Drop`.
// - control_plane/handler/fetch_handler.rs + response_guard.rs: the
//   application answers once with `ok` or `error` (or the guard's
//   NOT_SUPPORTED when every handler clone is dropped: the same `respond(Error)`).
//
// Each action is one atomic step:
// - `appOkWrite`: `send_on_incoming(FETCH_OK)`: clones the entry under the
//   `requests` lock and writes FETCH_OK; it fails when the entry is gone or
//   the peer stopped the stream. `appOkAdvance`: the following
//   `advance_fetch(fetch_ok_sent)` lock hold; when the response already ended
//   it removes the entry and FINs the request stream.
// - `appError`: `send_on_incoming(REQUEST_ERROR, ends_request)`: takes the
//   entry and writes REQUEST_ERROR + FIN (the write fails after the peer's
//   STOP_SENDING). With RESET_RESPONSE_ON_ERROR it first resets an attached
//   response stream still open (`ServedFetch::ResponseAttached`).
// - `openResponse`: `create_fetch_stream` / `FetchDataSender::new`:
//   `attach_fetch_response` under the lock, or a reset and bail when the
//   entry is gone (ATTACH_CHECKS_ENTRY).
// - `closeResponse` / `resetResponse` / `dropResponse`: `close` (sets
//   `ended`, checks `stopped()` once, FINs), `reset`, and `Drop` (a spawned
//   reset). Each that reaches the stream leaves a pending
//   `fetch_response_ended`, run by `reportEnded`. A `close` or `reset` of a
//   stream the cancel path already reset fails after setting `ended`, so it
//   reports nothing.
// - `sendObjectStopped`: `send_object` seeing the peer's STOP_SENDING on the
//   response stream runs `fetch_cancelled`.
// - `peerStop`: the requester cancels: STOP_SENDING on the request stream
//   (a MUST in §5.2), optionally on the response stream too.
// - `noticeStop`: the request task's `sender.stopped()` (or the peer's
//   reset, `peer_ended`) runs `fetch_cancelled` → `reset_cancelled_fetch`:
//   takes the entry and resets the request stream and an attached response.
//
// `draft18_fetch_current` is the implementation, with RESET_RESPONSE_ON_ERROR
// on, and passes. Guards the implementation
// relies on:
// - `draft18_fetch_attach_unchecked`: ATTACH_CHECKS_ENTRY off fails
//   `noOrphanResponse`.
// - `draft18_fetch_fin_at_ok`: WAIT_FOR_RESPONSE off (FETCH_OK ends the
//   request stream at once, like TRACK_STATUS) fails
//   `requestFinOnlyAfterResponse`.
// - `draft18_fetch_close_may_fail`: a FIN that fails on an open stream
//   (CLOSE_MAY_FAIL) fails `noLeakedEntry`: `close` sets `ended` before the
//   FIN, so neither it nor `Drop` reports the end. The implementation passes
//   only because its transports never fail that FIN.
//
// Check (from docs/architecture/moqt/models, Java 21+):
//   quint typecheck draft18_fetch.qnt
//   quint verify --backend tlc --main draft18_fetch_current --invariant safety draft18_fetch.qnt   # passes
//   quint verify --backend tlc --main draft18_fetch_attach_unchecked --invariant safety draft18_fetch.qnt
//   quint verify --backend tlc --main draft18_fetch_fin_at_ok --invariant safety draft18_fetch.qnt
//   quint verify --backend tlc --main draft18_fetch_close_may_fail --invariant safety draft18_fetch.qnt
//   quint verify --backend tlc --main draft18_fetch_without_reset_response_on_error --invariant safety draft18_fetch.qnt
module draft18_fetch {
  const RESET_RESPONSE_ON_ERROR: bool
  const ATTACH_CHECKS_ENTRY: bool
  const WAIT_FOR_RESPONSE: bool
  // A transport whose FIN can fail on a stream this side has not reset.
  // quinn's `SendStream::finish` (and WebTransport's) fails only on a stream
  // already finished or reset, and returns Ok on one the peer stopped.
  const CLOSE_MAY_FAIL: bool

  // The `incoming` entry of the FETCH and its `ServedFetch`.
  type Entry =
    | EDelivered       // `Open` or `ResponseAttached`
    | EOkSent          // `OkSent`
    | EResponseEnded   // `ResponseEnded`
    | ERemoved

  // This side's half of the request stream.
  type Half = HOpen | HFin | HReset

  // The response (FETCH data) stream.
  type Resp = RNone | ROpen | RFin | RReset

  // The `FetchDataSender` value.
  type DataSender =
    | DNone     // not created, or `new` bailed
    | DLive     // `ended` false
    | DEnded    // `close` or `reset` was called
    | DDropped  // dropped while `ended` was false

  // A `fetch_response_ended` call.
  type Report = NoReport | ReportPending | Reported

  // The application's answer.
  type Answer =
    | AUnanswered
    | AOkWritten   // FETCH_OK written, `advance_fetch` not run yet
    | AOk
    | AError       // REQUEST_ERROR taken the entry
    | AFailed      // the answer could not be written

  type State = {
    entry: Entry,
    // `ServedFetch` holds the response stream.
    attached: bool,
    half: Half,
    resp: Resp,
    sender: DataSender,
    report: Report,
    answer: Answer,
    peerStopped: bool,
    peerStoppedData: bool,
    noticed: bool,
  }

  var s: State

  action init = s' = {
    entry: EDelivered,
    attached: false,
    half: HOpen,
    resp: RNone,
    sender: DNone,
    report: NoReport,
    answer: AUnanswered,
    peerStopped: false,
    peerStoppedData: false,
    noticed: false,
  }

  def present: bool = s.entry != ERemoved

  // `fetch_cancelled`: takes the entry and resets both streams.
  def cancelled(st: State): State =
    if (st.entry == ERemoved) st
    else {
      ...st,
      entry: ERemoved,
      half: HReset,
      resp: if (st.attached and st.resp == ROpen) RReset else st.resp,
    }

  action appOkWrite = all {
    s.answer == AUnanswered,
    if (present and not(s.peerStopped) and s.half == HOpen)
      s' = { ...s, answer: AOkWritten }
    else
      s' = { ...s, answer: AFailed },
  }

  action appOkAdvance = all {
    s.answer == AOkWritten,
    s' = {
      ...s,
      answer: AOk,
      entry: match s.entry {
        | EDelivered => if (WAIT_FOR_RESPONSE) EOkSent else ERemoved
        | EResponseEnded => ERemoved
        | _ => s.entry
      },
      half: if (s.entry == EResponseEnded or (s.entry == EDelivered and not(WAIT_FOR_RESPONSE))) HFin
            else s.half,
    },
  }

  action appError = all {
    s.answer == AUnanswered,
    if (not(present))
      s' = { ...s, answer: AFailed }
    else
      s' = {
        ...s,
        answer: AError,
        entry: ERemoved,
        half: if (s.peerStopped) s.half else HFin,
        resp: if (RESET_RESPONSE_ON_ERROR and s.attached and s.resp == ROpen) RReset else s.resp,
      },
  }

  action openResponse = all {
    s.resp == RNone,
    if (present or not(ATTACH_CHECKS_ENTRY))
      s' = { ...s, attached: present, resp: ROpen, sender: DLive }
    else
      s' = { ...s, resp: RReset },
  }

  action closeResponse = all {
    s.sender == DLive,
    if (s.resp == ROpen and s.peerStoppedData)
      s' = { ...cancelled(s), sender: DEnded }
    else if (s.resp == ROpen)
      any {
        s' = { ...s, sender: DEnded, resp: RFin, report: ReportPending },
        // `close` sets `ended`, then `?` returns the FIN error before
        // `response_ended`.
        all { CLOSE_MAY_FAIL, s' = { ...s, sender: DEnded } },
      }
    else
      // The stream was reset already: `close` fails after setting `ended`.
      s' = { ...s, sender: DEnded },
  }

  action resetResponse = all {
    s.sender == DLive,
    if (s.resp == ROpen)
      s' = { ...s, sender: DEnded, resp: RReset, report: ReportPending }
    else
      s' = { ...s, sender: DEnded },
  }

  action dropResponse = all {
    s.sender == DLive,
    s' = { ...s, sender: DDropped, resp: if (s.resp == ROpen) RReset else s.resp, report: ReportPending },
  }

  action sendObjectStopped = all {
    s.sender == DLive,
    s.resp == ROpen,
    s.peerStoppedData,
    s' = cancelled(s),
  }

  action reportEnded = all {
    s.report == ReportPending,
    s' = {
      ...s,
      report: Reported,
      entry: match s.entry {
        | EDelivered => EResponseEnded
        | EOkSent => ERemoved
        | _ => s.entry
      },
      half: if (s.entry == EOkSent) HFin else s.half,
    },
  }

  action peerStop = all {
    not(s.peerStopped),
    s.half == HOpen,
    nondet data = oneOf(Set(true, false))
    s' = { ...s, peerStopped: true, peerStoppedData: data and s.resp == ROpen },
  }

  action noticeStop = all {
    s.peerStopped,
    not(s.noticed),
    s' = { ...cancelled(s), noticed: true },
  }

  action step = any {
    appOkWrite,
    appOkAdvance,
    appError,
    openResponse,
    closeResponse,
    resetResponse,
    dropResponse,
    sendObjectStopped,
    reportEnded,
    peerStop,
    noticeStop,
  }

  // §5.2: after FETCH_OK the request stream ends with FIN only once the
  // response stream has ended (FIN or reset): the requester keeps its state
  // until then.
  val requestFinOnlyAfterResponse: bool =
    (s.half == HFin and (s.answer == AOk or s.answer == AOkWritten)) implies
      (s.resp == RFin or s.resp == RReset)

  // A response stream still open has its FETCH entry, so a later cancel
  // (STOP_SENDING) resets it as §5.2 requires and the stream does not outlive
  // the FETCH's state (REQUEST_ERROR: "both endpoints can immediately destroy
  // state").
  val noOrphanResponse: bool = s.resp == ROpen implies present

  // Once FETCH_OK is out and the response stream has ended and reported so,
  // the entry is gone and the request stream finished: nothing is left
  // waiting for a peer cancel that a finished FETCH never gets.
  val noLeakedEntry: bool =
    (s.answer == AOk and (s.sender == DEnded or s.sender == DDropped) and s.report != ReportPending)
      implies not(present)

  // The request stream is ended (FIN or reset) at most by the remover of the
  // entry: an entry still present has its stream open.
  val presentEntryHasOpenStream: bool = present implies s.half == HOpen

  // §5.2: once the publisher has acted on the requester's STOP_SENDING it
  // MUST reset the response stream; it never stays open.
  val stopResetsResponse: bool = s.noticed implies s.resp != ROpen

  val safety: bool = and {
    requestFinOnlyAfterResponse,
    noOrphanResponse,
    stopResetsResponse,
    noLeakedEntry,
    presentEntryHasOpenStream,
  }
}

// expect: ok
module draft18_fetch_current {
  import draft18_fetch(
    RESET_RESPONSE_ON_ERROR = true,
    ATTACH_CHECKS_ENTRY = true,
    WAIT_FOR_RESPONSE = true,
    CLOSE_MAY_FAIL = false,
  ).*
}


// expect: violation
module draft18_fetch_attach_unchecked {
  import draft18_fetch(
    RESET_RESPONSE_ON_ERROR = true,
    ATTACH_CHECKS_ENTRY = false,
    WAIT_FOR_RESPONSE = true,
    CLOSE_MAY_FAIL = false,
  ).*
}

// expect: violation
module draft18_fetch_fin_at_ok {
  import draft18_fetch(
    RESET_RESPONSE_ON_ERROR = true,
    ATTACH_CHECKS_ENTRY = true,
    WAIT_FOR_RESPONSE = false,
    CLOSE_MAY_FAIL = false,
  ).*
}

// expect: violation
module draft18_fetch_close_may_fail {
  import draft18_fetch(
    RESET_RESPONSE_ON_ERROR = true,
    ATTACH_CHECKS_ENTRY = true,
    WAIT_FOR_RESPONSE = true,
    CLOSE_MAY_FAIL = true,
  ).*
}

// expect: violation
module draft18_fetch_without_reset_response_on_error {
  import draft18_fetch(
    RESET_RESPONSE_ON_ERROR = false,
    ATTACH_CHECKS_ENTRY = true,
    WAIT_FOR_RESPONSE = true,
    CLOSE_MAY_FAIL = false,
  ).*
}

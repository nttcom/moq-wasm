# `moqt` State Machines

Catalog of the lifecycles in `crates/moqt` that the code encodes implicitly (field combinations, map membership, drop guards, task locals, await order) and the explicit state each one stands for. Paths are relative to `crates/moqt/src/modules/`.

Each entry names the explicit states. Where a Quint model exists under [`models/`](models/), the model's sum type is the reference shape for an explicit Rust enum, and its `safety` invariant is what a refactor must keep. Run the models with [`models/README.md`](models/README.md).

**Legend** — *Encoding*: where the state lives today. *Model*: Quint file (`—` = not modeled; sequential or trivially explicit).

## Already explicit

| Enum | Location |
| --- | --- |
| `Publication {Subscribed, Published, Released}` | `moqt/domains/origin.rs` |
| `ObjectSink {Buffer, Receiver, Cancelled}` | `moqt/domains/connection.rs` |
| `Cutover {Waiting, At}` | `moqt/domains/group_cutover.rs` |
| `Entry {Waiting, Abandoned}` (draft-14 inflight request), `LateResponseAction` | `moqt/version/draft14.rs` |
| `AnswerSlot {Waiting, Answered, TimedOut}` | `moqt/control_plane/handler/update_answer.rs` |
| `Outstanding {PassedOn, Delayed}` | `moqt/domains/origin/new_group_requests.rs` |
| `Carried {Moved, Ends, Stays, Gone}` (outcome, not stored) | `moqt/domains/session/migration_task.rs` |
| `client::subscription::Reading {Unstarted, Objects, Groups}` | `client/subscription.rs` |
| `StreamDataSender<Uninitialized \| HeaderSent>` (typestate) | `moqt/data_plane/stream/stream_data_sender.rs` |
| `DecoderState {Start, Subgroup, Fetch}` | `moqt/version/draft18/objects.rs` |

## Origin (original publisher)

| Machine | Encoding | Explicit states | Model |
| --- | --- | --- | --- |
| Track | `OriginState.tracks[key]` presence, `ended`, `datagrams` + `numbering.largest()` | `Unregistered`, `Live{delivery: Undecided \| Subgroup \| Datagram}`, `Ended` | [`origin.qnt`](models/origin.qnt) |
| Outlet (per subscription) | `Outlet{publication, start, forward, send_task}` + `state.subscriptions` membership | `Serving{Subscribed \| Published, forward, start}`, `Released`, `Gone` | [`origin.qnt`](models/origin.qnt), [`origin_sessions.qnt`](models/origin_sessions.qnt) |
| Outlet send task | channel open/closed, `CancellationToken`, task position | `Idle`, `Sending`, `Ending`, `Cancelling`, `Exited` | — |
| `TrackWriter` group | `group`, `abandoned`, `next_group_id`, `first_group_id`, `pending_group_gap` | `Fresh`, `Open(g)`, `Abandoned(g)`, `Between`, `Exhausted`, `Consumed` × `PendingGap(n)` | [`track_writer.qnt`](models/track_writer.qnt) |
| Forward State per outlet | `Outlet.forward`, `start`, writer `abandoned` | `InGroup`, `LeftGroup(g)`, `WaitingNextGroup`, `NotStarted` | [`origin_sessions.qnt`](models/origin_sessions.qnt) |
| Demand (on-demand publishing) | `demand: Option<Demand{reported}>` | `NotOnDemand`, `OnDemand{Wanted \| Unwanted}` | [`origin_sessions.qnt`](models/origin_sessions.qnt) |
| NEW_GROUP_REQUEST throttle | `new_group_requests: Option<_>`, `open_group`, `outstanding`, delay task | `Disabled`, `NoGroupSeen`, `Idle`, `Delayed`, `PassedOn` | [`new_group_requests.qnt`](models/new_group_requests.qnt) |
| Cache expiry | `cache.entries`, `cache_expiry: Option<CacheExpiryTask>` | `Empty`, `Holding{Armed(at) \| Unarmed}` | — |
| FETCH serve task | `OriginState.fetches[key]` presence | `Serving`, `Absent` | — |
| `TrackReader` read task | task position, channel closed | `AwaitingReceiver`, `FetchingPrefix`, `Reading`, `Draining`, `Done`; `awaiting_new_group`: `Waiting{held}`, `FetchingFallback`, `Live` | — |

## Subscriber side

| Machine | Encoding | Explicit states | Model |
| --- | --- | --- | --- |
| Track-alias sink | explicit: `ObjectSink` (`Buffer` / `Receiver` / `Cancelled`) in `moqt/domains/connection.rs`; absent `ObjectSinks.sinks[alias]` = `Unknown`, closed `SinkReceiver.sender` = `ReceivingOrphaned`; `pending_objects` and `expiries` are private to `ObjectSinks` | `Unknown`, `Buffering(age, n)`, `Receiving`, `ReceivingOrphaned`, `Cancelled(age)` | [`object_sinks.qnt`](models/object_sinks.qnt) |
| Subscription | version-layer request entry, sink, `receiver_map`, carried record, `PublishDoneExpiryTask.waiting` | `Reserving`, `AwaitingOk`, `OkUnregistered`, `ActiveUnclaimed`, `ActiveClaimed`, `DoneDraining`, `Cancelled`, `Rejected`, `Abandoned` | [`subscription.qnt`](models/subscription.qnt) |
| PUBLISH_DONE countdown | `PublishDoneExpiryTask.waiting`, `SinkReceiver.arrived_streams` | `Idle`, `Counting(n)`, `GraceOnly`, `Expired` | [`subscription.qnt`](models/subscription.qnt) |
| Subscriber Forward / update | `Subscription.forward`, `last_update`, carried `option.forward` | `Original(forward)`, `Updated(option)` | — |
| Pending FETCH | `fetch_notification_map`, `fetch_receiver_map`, `PendingFetch.kept` | `Registered`, `StreamDelivered`, `Kept`, `Accepted`, `Removed`, `ConnectionClosed` | — |
| PUBLISH_NAMESPACE record | `published_namespaces`, carried `published_namespaces`, `PendingPublishNamespace.record` | `Pending`, `Accepted`, `Rejected`, `Withdrawn`, `Cancelled` | [`publish_namespace.qnt`](models/publish_namespace.qnt) |
| SUBSCRIBE_NAMESPACE (carried) | carried `namespaces[id]` | `InFlight`, `Carried`, `Removed` | — |
| `NamespaceWatchers` entry | `Vec<(prefix, Sender)>` | `Active`, `Dropped` (pruned lazily) | — |

## Version layer — draft-14

| Machine | Encoding | Explicit states | Model |
| --- | --- | --- | --- |
| Outgoing request | `InflightRequests` (`Open(id → Entry)` / `Closed`) and `Entry` (`Waiting` / `Abandoned`) in `version/draft14.rs`, `RegisteredSender` / `ReceivedResponse` drop guards | `Unregistered`, `Waiting`, `Abandoned`, `Delivered`, `Accepted`, `Withdrawing`, `Failed` | [`draft14_requests.qnt`](models/draft14_requests.qnt) |
| Outbound Request ID credit | `OutgoingRequestIds{next, blocked: RequestsBlockedSent}` in `version/draft14.rs`, `peer_request_id_limit`, reservation guard | `Free`, `WaitingForCredit`, `BlockedNotified`, `Reserved`, `Consumed`, `Closed` | [`draft14_requests.qnt`](models/draft14_requests.qnt) |
| Inbound Request ID window | `next_peer_request_id`, `request_id_limit` | `InWindow`, `NeedsRaise`, `Violation` | — |
| GOAWAY received | `Connection.go_away_received` | `NotReceived`, `Received`, `Violation` | — |

## Version layer — draft-18

| Machine | Encoding | Explicit states | Model |
| --- | --- | --- | --- |
| Outgoing request stream | `requests.outgoing[id]`; caller side `RequestCancel(CallerState)` (`AwaitingResponse` / `Delivered` / `Ended`) in `version/draft18.rs`; task side `FirstMessage` in `version/draft18/request_streams.rs` | `Opening`, `AwaitingResponse`, `Established`, `Rejected`, `Ended` | [`draft18_outgoing_request.qnt`](models/draft18_outgoing_request.qnt) |
| Incoming request stream | task locals `reading`, `watching_stop`, `go_away_received`, `kind`/`updated_kind`; `requests.incoming[id]` | `Accepting`, `Open`, `AnsweredReading`, `HalfClosed`, `Ended` | — |
| Serving FETCH | `ServedFetch` (`Open` / `ResponseAttached` / `OkSent` / `ResponseEnded`) in `version/draft18.rs`, entry presence | `Open`, `ResponseAttached`, `OkSent`, `ResponseEnded`, `Done`, `Cancelled`, `Rejected` | [`draft18_fetch.qnt`](models/draft18_fetch.qnt) |
| REQUEST_UPDATE | outbound `UpdateWaiters` queue (`enqueue` / `withdraw_unwritten` / `answer_front`) and per-update typestate `QueuedUpdate` → `UpdateAnswer` in `version/update_answer.rs`; inbound `FuturesOrdered` + `AnswerSlot` | per update: `Queued`, `Written`, `Answered`, `TimedOut` | [`draft18_request_update.qnt`](models/draft18_request_update.qnt) |
| Namespace subscription / advertisement | `held_namespaces`, `accepted`, `advertised`, `blocked_tracks` | `Unanswered{held}`, `Accepted`, `Removed`; per track `Blocked` | — |
| Registry binding | `binding: OnceLock` | `Unbound`, `Bound` | — |

## Control plane

| Machine | Encoding | Explicit states | Model |
| --- | --- | --- | --- |
| Handler answer (`ResponseGuard`) | `Answer` enum in `answer: Arc<Mutex<Answer>>` + `Arc::strong_count` (`handler/response_guard.rs`) | `Unanswered`, `Answered`, `AutoRejected` | [`response_guard.qnt`](models/response_guard.qnt) |

## Session, migration and client

| Machine | Encoding | Explicit states | Model |
| --- | --- | --- | --- |
| Migration router | `Router::route` locals `previous`, `migration`, `reconnecting`, `current_closed` | `Steady`, `Draining`, `Migrating`, `MigratingCurrentLost`, `Reconnecting`, `Ended` | [`migration.qnt`](models/migration.qnt) |
| Connection | `go_away_received`, `closed_locally`, `served`, membership in `connections` | `Live`, `GoAwayReceived`, `ClosedLocally`, `ClosedByPeer`, `Retired` | [`migration.qnt`](models/migration.qnt) |
| Group cutover | `GroupCutover{newest, current_start, state}`, `cut_over` sender | `Waiting(newest)`, `PreviousEnded`, `At(c)` × `Signalled` | [`group_cutover.qnt`](models/group_cutover.qnt) |
| Session close | `UnacknowledgedStreams`, no session flag | `Open`, `Closing`, `Closed` | — |
| Client event loop (`Routes`) | `EventLoopTask` handle, route tables | `Running`, `Ended`, `Closed` | — |
| `TrackFollower` | `followed: Option`, `announced_while_following` | `Idle`, `Following`, `FollowingWithPendingHandover` | [`track_follower.qnt`](models/track_follower.qnt) |
| `TrackWriter` subscriber notifications (client) | `subscribers: Arc<Semaphore>` | `Open`, `Closed`, `Unregistered` | — |

## Data plane and transport

| Machine | Encoding | Explicit states | Model |
| --- | --- | --- | --- |
| `StreamDataSender<HeaderSent>` | `payload_remaining`, per-connection `streams` | `BetweenObjects`, `InPayload`, `Closed` | — |
| `StreamDataReceiver` | `header_returned`, `first_part`, `open`, `arriving` | `AwaitHeader`, `HeaderPeeked`, `Objects`, `InPayload`, `Ended` | — |
| `FetchDataReceiver` | `carry`, `request_end`, `ledger` | `Reading`, `Resumed`, `Ended`, `Failed` | — |
| `ObjectLedger` entry | map per `Location` | `Absent`, `Recorded` | — |
| QMux session / streams | `closed` watch, `ended`, `SendQuota`, receive counters | session `Handshaking`, `Open`, `Closed`; send `Open`, `Finished`, `Reset`, `Stopped`, `Lost`; receive derived from `final_size`/`reset_code` | — |

## Verification results

Every `*_current` instance except `origin_current` violates `safety`: TLC found a counterexample in today's code. Each defect below has a fix switch, and `*_fixed` (all of a model's switches on) holds over every reachable state. `check.sh` reproduces the table.

| Model | Defect (counterexample in `*_current`) | Fix switch |
| --- | --- | --- |
| `origin_sessions` | A session that is both a subscriber and a PUBLISH peer of one track gets two serving outlets (SUBSCRIBE registered while `publish` awaits PUBLISH_OK); every object is sent twice | `RECHECK_ON_REGISTER` + `SUBSCRIBE_ENDS_PUBLISH` |
| `track_writer` | Prior Group ID Gap covers groups that exist: groups passed under Forward State 0, or a group whose stream failed to open | `GAP_FROM_NUMBERING` |
| `new_group_requests` | Enabling dynamic groups after writes passes the first NEW_GROUP_REQUEST through unthrottled; re-enabling drops a delayed request | `KEEP_ON_REENABLE`, `SEED_OPEN_GROUP` |
| `object_sinks` | `install` drains a buffer older than `UNKNOWN_TRACK_ALIAS_PERIOD` (expiry runs only in `notify`/`cancel`) | `EXPIRE_ON_INSTALL` |
| `subscription` | PUBLISH_DONE right behind SUBSCRIBE_OK is processed before the receiver registers: full 10 s grace, stale carried record, or a receiver that never ends; expiry removes an unclaimed receiver (objects lost); client `done()` registered after the event loop routed PUBLISH_DONE or after `end()` never resolves; updates sent for an ended request | `ACCEPT_BEFORE_NEXT`, `DONE_BEFORE_SUBSCRIBE`, `KEEP_UNCLAIMED_RECEIVER`, `END_TRACKED` |
| `publish_namespace` | Dropped `PendingPublishNamespace` keeps its record; PUBLISH_NAMESPACE_CANCEL changes no state; a namespace refused during a migration is re-announced on the new connection | `FORGET_ON_DROP`, `FORGET_ON_CANCEL`, `WITHDRAW_ELSEWHERE` |
| `draft14_requests` | A request whose write failed or was cancelled is counted as abandoned; a cancelled partial `write_all` leaves a torn control message | `UNREGISTER_UNSENT`, `FINISH_CANCELLED_WRITE` |
| `response_guard` | `accept`/`reject` do not check `answer` (two answers); two clones dropped concurrently both skip the NOT_SUPPORTED auto-reject (no answer) | `ANSWER_ONCE`, `LAST_DROP_ATOMIC` |
| `draft18_outgoing_request` | PUBLISH_NAMESPACE answered locally does not consume its Request ID (two requests share it); a SUBSCRIBE_OK dropped before `accept` leaves its track alias sink; a malformed or missing first response reports `SessionClosed` instead of closing the session | `CONSUME_LOCAL`, `CLEAN_ABANDONED_ALIAS`, `FIX_FIRST_MESSAGE` |
| `draft18_request_update` | A REQUEST_UPDATE send cancelled mid-await leaves its waiter queued, so later REQUEST_OKs reach the wrong update (or a torn message) | `WRITE_DETACHED` |
| `draft18_fetch` | REQUEST_ERROR after the response stream is attached leaves that stream open (also after the peer's STOP_SENDING, §5.2) | `RESET_RESPONSE_ON_ERROR` |
| `migration` | A GOAWAY migration in flight when the current connection closes keeps the dead connection as `previous` forever, and revives a locally closed session; `Carried::Stays` subscriptions are cut silently; PUBLISH_DONE can be written after `previous` is closed; an UNSUBSCRIBE before `serve()` leaves a stale served entry; a session ending with a `previous` leaves it open | `FIX_CLOSED_DURING_MIGRATION`, `FIX_FINISH_CLOSES_PREVIOUS`, `FIX_REPORT_STAYING_END`, `FIX_DONE_BEFORE_STOP`, `FIX_SERVE_ON_RECEIPT` |
| `group_cutover` | A group the current connection delivered whole is dropped when its streams arrive out of order; a lagging relay's groups are delivered twice after `previous_ended` | `CUT_FROM_START`, `KEEP_NEWEST` (trade-off in file) |
| `track_follower` | A withdrawal (`active=false`) keeps the handover flag, so the follower resubscribes to a gone publisher; `TrackWatch::next` never returns `None` after the session ends | `CLEAR_ON_WITHDRAWAL`, `END_DROPS_HANDLERS` |

Refuted by the models: Origin outlets surviving a disconnect (one `Origin` per client session; `retire` releases them), Forward State demand drift, `group` and `abandoned` both set in `TrackWriter`, `ended` before a failing `close()` leaking a FETCH entry (quinn's `finish` does not fail there), and PUBLISH_NAMESPACE_DONE racing an in-flight PUBLISH_NAMESPACE (the peer sees them in order).

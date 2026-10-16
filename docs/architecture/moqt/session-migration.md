# Session migration inside `Session`

## Status

Accepted

## Date

2026-10-16

## What

A client `Session` follows the server's GOAWAY by itself (draft-14 §3.5:
"Ideally this is transparent to the application using MOQT"). The application
keeps its `Session`, `Subscription`s, data receivers, stream senders and
handlers; request ids and track aliases it has seen keep their values.
`MigratingSession` and the browser's `moqtClient.ts` switchover are removed.

## Context

Migration lives in three places today:

| Where | Subscribing side | Publishing side |
| --- | --- | --- |
| `moqt::MigratingSession` (#799, #804) | `MigratingTrack`, object level | `Origin` with one outlet per session (#803) |
| `broadcast::RelayConnection` | — | through `MigratingSession` (#805) |
| `examples/browser/lib/moqt/moqtClient.ts` | id/alias maps, group cutover, joining FETCH fill, FETCH carry-over, Forward | dual send for PUBLISH and for SUBSCRIBE-served tracks |

The wasm `MOQTClient` exposes a message-level API: JavaScript chooses request
ids, reads subgroup headers, objects and ends per track alias, and writes
subgroups by track alias. `MigratingSession` hides ids behind handles and
delivers objects only, so the browser cannot use it without an id map of its
own. Putting the id map into `Session` serves Rust and the browser alike.

## Design

### Layering

`Session` becomes the logical session. The per-connection state of
`SessionContext` moves to a `Connection` (transport, `ControlChannel` with
its request id allocator, the track aliases this side assigns, GOAWAY state,
the object sinks keyed by wire alias and the FETCH stream notifications keyed
by wire request id), and the tasks that read a connection are bound to it
(`ConnectionTasks`). `Session` holds:

- the current `Connection` and, during a migration, the previous one;
- the logical request table (below);
- one event channel fed by the connections;
- a handle to the `Endpoint`'s connection creator, so the session can open
  the new connection on the same socket and transport (`BROWSER` included);
- the `MigrationTask`, which reacts to GOAWAY.

Requests sent while a migration re-sends the others wait for it, so none is
missed.

`Publisher`, `Subscriber` and the handlers hold the logical session and pick
the connection when a request is sent, not when the handle is created. A
handler for an incoming request keeps the connection the request arrived on,
so its answer goes there.

### Turning it off

`ClientConfig::follow_go_away: bool`. With `false` the session behaves as
today: GOAWAY is a `SessionEvent` and nothing migrates. A server-side session
never migrates.

| Client | Setting | Why |
| --- | --- | --- |
| relay → relay (`inter_relay_connection_manager`) | `false` | a relay-peer GOAWAY is answered by subscribing other publishers ("GOAWAY sequence") |
| relay stats publisher (`stats_publish_task`) | `false` | it connects to its own relay, which is the one draining |
| broadcast, recorder, observability, live-egress, onvif-ingest command subscriber, moq-cli, wasm | `true` | |

The default is `true`; the two relay call sites set `false` in the same
change.

### Request ids and track aliases

Each connection allocates its own wire ids, as draft-14 §9.1 requires. The
application sees logical ids:

- **Requests this side sends** (SUBSCRIBE, SUBSCRIBE_NAMESPACE, PUBLISH,
  PUBLISH_NAMESPACE, FETCH, TRACK_STATUS): the logical id is the wire id on the
  connection the request was first sent on. Each connection keeps
  `logical → wire` for the requests it carries, used for UNSUBSCRIBE,
  SUBSCRIBE_UPDATE, FETCH_CANCEL, PUBLISH_DONE and responses.
- **Requests the peer sends** (SUBSCRIBE, PUBLISH, FETCH, TRACK_STATUS, …):
  the first connection uses the wire id as the logical id. A later connection
  maps each wire id to a fresh logical id from a session-wide counter, so ids
  from the two relays never collide in a map the application keys by request
  id (the wasm `IncomingRequests`).
- **Track aliases**: the logical alias is the alias of the first connection
  (assigned by the peer for SUBSCRIBE, by this side for PUBLISH). A later
  connection translates its wire alias to the logical one where objects enter
  (`SubscriptionNotifier::notify`) and where streams and datagrams are opened.

The JavaScript API keeps its shape: the ids it passes and receives are logical
ids.

### Subscribing side

A subscription carried over is re-sent with `NextGroupStart` (or with its
Forward state and its original range for a finite one). Its incoming streams
of both connections feed the same `StreamDataReceiverFactory`. A subgroup
stream carries one group, so the group cutover of `GroupCutover` becomes a
per-stream decision: the factory hands out the streams of groups before the
cutover from the previous connection and the rest from the new one, and drops
the others before the application sees them. Datagrams take the same
decision per object. `TrackReader`, `MigratingTrack`'s users and the wasm
`track_io` all read the factory, so they need no change.

A FETCH in flight on the previous connection keeps reading there; when that
stream is cut off after the move, the part not delivered yet (the range
after the last location received, up to the FETCH_OK's End Location) is
FETCHed on the current connection and the same receiver reads on. A
descending FETCH is not sent again.

### Publishing side

PUBLISH_NAMESPACE and PUBLISH are sent again on the new connection. A
`StreamDataSenderFactory` (and `DatagramSender`) of a logical publication
opens its streams on every connection whose peer still takes the track: the
new one from the next group, the previous one until the peer UNSUBSCRIBEs
there (draft-22 §7.6.1). This covers both PUBLISH-initiated tracks and tracks
served for a SUBSCRIBE the previous relay sent. With fan-out below `Origin`,
the per-session outlets of #803 are no longer needed and `Origin` goes back
to one logical session.

### Events

`receive_event` returns the events of every connection with logical ids.
GOAWAY is consumed when `follow_go_away` is on. `Disconnected` comes once, when
the current connection closed and no migration replaced it. The previous
connection's Disconnected and ProtocolViolation are dropped. A
PUBLISH_NAMESPACE the new relay sends for a namespace already announced on the
previous one is delivered as is (the application acknowledges it again, as
the recorder does now).

### Closing the previous connection

With NO_ERROR once every subscription cut over (5 s grace, 10 s limit for a
track that delivers nothing on the new connection) and the peer takes no
track this side sends there, or when the server closes it. A SUBSCRIBE the
previous server sent and never unsubscribed ends then with an `Unsubscribe`
event, as `moqtClient.ts` reports it today.

## What `moqtClient.ts` does that this design covers

| `moqtClient.ts` | Here |
| --- | --- |
| app alias ↔ wire alias, request ids kept | logical ids and aliases in `Session` |
| group cutover, `previousNewestGroup` | `GroupCutover` per stream in the receiver factory |
| joining FETCH to fill the new relay's current group | not needed: the previous connection supplies every group before the cutover; a previous connection lost before the cutover leaves the gap of today |
| FETCH remainder carried over | FETCH carry-over, ascending only: a descending FETCH cut off is not sent again |
| Forward=0 carried | Forward state re-sent with the SUBSCRIBE |
| dual send for PUBLISH and SUBSCRIBE-served tracks | fan-out in the sender factory |
| retry every 500 ms for 10 s | same |
| leave the old relay at once when no subgroup is arriving there (`onSubgroupEnd`) | not covered: the previous connection is kept until every carried subscription cut over (at most 10 s) and 5 s more, so a quiet track such as chat keeps a draining relay up to 15 s |

## Alternatives

- **Keep `MigratingSession` and add an id map in wasm.** The map then exists
  in wasm and, once Rust applications want stable ids, again in `moqt`.
- **Keep the TypeScript switchover.** Migration stays in two languages.

## Steps

Each step is one PR, stacked:

1. **Connections under `Session`, no migration.** `SessionContext` splits into
   the logical session and `Connection`, the tasks and object sinks of a
   connection are bound to it, and each handler answers on the connection
   its request arrived on; logical ids equal wire ids. No behaviour change;
   the existing tests are the check.
2. **`follow_go_away` and subscribing-side migration in `Session`.** Logical id
   maps, per-stream cutover, events. The relay, `broadcast` and wasm set
   `false` until their steps. The recorder moves from `MigratingSession` to
   `Session`.
3. **Publishing-side migration in `Session`.** Logical ids for the requests
   the peer sends; PUBLISH_NAMESPACE and PUBLISH carried and fan-out in the
   sender factory; `broadcast` moves to `Session`; `Origin` keeps an outlet
   per subscription instead of per session; `MigratingSession` is removed.
4. **FETCH carry-over and Forward state.**
5. **wasm and `moqtClient.ts`.** The wasm client turns `follow_go_away` on and
   the TypeScript switchover is deleted; `run-goaway-e2e.mjs` and the browser
   unit tests are the check.

## Open questions

- #803–#805 add the `Origin` outlets and `MigratingSession`'s publishing side,
  which step 3 removes again. They work today; merging them gives the Rust
  publishers drain support now, at the cost of code that is replaced later.
- The default of `follow_go_away`. `true` makes every client migrate unless it
  opts out, which is what draft-14 §3.5 recommends, but a client this
  repository does not know about changes behaviour.
- The migration connects with the token the session started with. A client
  that refreshes its token in-session (the recorder's `--auth-token-file`)
  needs the current token handed to the session.

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
It replaced `moqt::MigratingSession` and the browser's TypeScript switchover,
which are deleted.

## Context

`MigratingSession` hid request ids behind handles and delivered objects only.
The browser's API is message-level (JavaScript sees request ids and track
aliases), so it could not use `MigratingSession` without an id map of its
own and migrated in TypeScript instead. Putting the id map into `Session`
serves Rust and the browser alike.

## Design

### Layering

`Session` is the logical session. The per-connection state lives in a
`Connection` (transport, `ControlChannel` with
its request id allocator, the track aliases this side assigns, GOAWAY state,
the object sinks keyed by wire alias and the FETCH stream notifications keyed
by wire request id), and the tasks that read a connection are bound to it
(`ConnectionTasks`). `Session` holds:

- the current `Connection` and, during a migration, the previous one;
- the logical request table (below);
- one event channel fed by the connections;
- a handle to the `Endpoint`'s connection creator, so the session can open
  the new connection on the same socket and transport (`BROWSER` included);
- the `MigrationTask`, which reacts to GOAWAY and tries the new connection
  every 500 ms for 10 s.

Requests sent while a migration re-sends the others wait for it, so none is
missed.

`Publisher`, `Subscriber` and the handlers hold the logical session and pick
the connection when a request is sent, not when the handle is created. A
handler for an incoming request keeps the connection the request arrived on,
so its answer goes there.

### Turning it off

`ClientConfig::follow_go_away: bool`, `true` by default. With `false`, GOAWAY
is a `SessionEvent` and nothing migrates. A server-side session never
migrates.

| Client | Setting | Why |
| --- | --- | --- |
| relay → relay (`inter_relay_connection_manager`) | `false` | a relay-peer GOAWAY is answered by subscribing other publishers ("GOAWAY sequence") |
| broadcast, recorder, egress-gateway, ingest-gateway pull command subscriber, moq-cli, wasm | `true` | |

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
  id.
- **Track aliases**: the logical alias is the alias of the first connection
  (assigned by the peer for SUBSCRIBE, by this side for PUBLISH). A later
  connection translates its wire alias to the logical one where objects enter
  (`subscription_notifier::notify`) and where streams and datagrams are opened.

### Subscribing side

A subscription carried over is re-sent with its Forward state and
`NextGroupStart`, or, for an AbsoluteRange, a range that starts after the
newest group the previous connection handed out and keeps its End Group.
A SUBSCRIBE_UPDATE replaces the carried Forward state and priority, and one
with an End Group turns the carried filter into an AbsoluteRange of its Start
Location and End Group, so a range narrowed after the SUBSCRIBE is kept. An
AbsoluteRange whose End Group the previous connection has started is not
re-sent: it ends there, and the previous connection is kept until its
streams end (10 s limit). A subscription that PUBLISH_DONE ended is not
sent again; a PUBLISH_DONE from the previous connection for a subscription
the current one carries only reports that server draining. A subscription
only delivers objects after the Largest Object of its SUBSCRIBE_OK
(draft-14 §9.7), so a range that starts at or below it joins that group
mid-way; `GroupCutover` leaves such a group to the previous connection. Its incoming streams
of both connections feed the same `DataReceiver`. A subgroup
stream carries one group, so the group cutover of `GroupCutover` becomes a
per-stream decision: the receiver hands out the streams of groups before the
cutover from the previous connection and the rest from the new one, and drops
the others before the application sees them. Datagrams take the same
decision per object. Each stream and datagram is checked by the draft of
the connection it arrived on, so a track moved from a draft-14 server to a
draft-18 one may mix subgroup streams and datagrams there. The track's
`ObjectLedger` belongs to the session, so an object received before the move
is still compared with its copies after it, a FETCH on the new connection
included.

A FETCH in flight on the previous connection keeps reading there; when that
stream is cut off after the move, the part not delivered yet (the range
after the last location received, up to the FETCH_OK's End Location) is
FETCHed on the current connection and the same receiver reads on. A
descending FETCH is not sent again.

A Joining FETCH may only join a Largest Object subscription (draft-14
§9.16.2), and a later connection holds the subscription with the filter it
was sent again with. So `fetch_relative_joining` sends a Joining FETCH only
on the connection the subscription was first sent on; once the session has
moved, it FETCHes the same range (from the joined group up to the
SUBSCRIBE_OK's Largest Location) as a Standalone FETCH.

### Publishing side

PUBLISH_NAMESPACE and PUBLISH are sent again on the new connection. A
`StreamDataSenderFactory` (and `DatagramSender`) of a logical publication
opens its streams on every connection whose peer still takes the track: the
new one from the next group, the previous one until the peer UNSUBSCRIBEs
there (draft-22 §7.6.1). This covers both PUBLISH-initiated tracks and tracks
served for a SUBSCRIBE the previous relay sent. The fan-out is below
`Origin`, which sees one logical session.

### Events

`receive_event` returns the events of every connection with logical ids.
GOAWAY is consumed when `follow_go_away` is on. `Disconnected` comes once, when
the current connection closed and no migration replaced it. The previous
connection's Disconnected and ProtocolViolation are dropped. A
PUBLISH_NAMESPACE the new relay sends for a namespace already announced on the
previous one is delivered as is (the application acknowledges it again, as
the recorder does).

### Reconnecting after a lost connection

A session that follows GOAWAY also replaces a connection that ended without
one, through the same migration to its current URL. It reconnects when the
network or the server ended the connection without a termination code, and
when the server closed it with EXPIRED_AUTH_TOKEN while
`ClientConfig::authorization_token` is a provider that hands out a token other
than the one just refused: a provider repeating it, as a token file nobody
refreshed does, would only be refused again, and the session ends instead. A
close this side made, and any other termination code, ends the session.

Attempts start 0.5 s after the loss and double their delay up to 10 s, for at
most 60 s, about as long as a relay keeps a track in its cache. A server that
rejects the token at SETUP (UNAUTHORIZED, EXPIRED_AUTH_TOKEN) gets one more
attempt only. Meanwhile `Session::closed` stays pending and no `Disconnected`
is delivered. A SUBSCRIBE_NAMESPACE, PUBLISH_NAMESPACE, PUBLISH or SUBSCRIBE
the loss cuts off before its answer (its send fails or the control stream
closes) releases the `carried` lock, waits for the new connection and is sent
there; it fails only when the session ends without one. A PUBLISH_NAMESPACE
or SUBSCRIBE_NAMESPACE is carried from the moment it was sent, so the
migration itself sends it again and `publish_namespace` or
`subscribe_namespace` returns once the new connection accepted it. A
PUBLISH_NAMESPACE or SUBSCRIBE_NAMESPACE that is rejected or times out
removes only its own record from `carried`, so another request for the same
namespace stays carried. An UNSUBSCRIBE_NAMESPACE or PUBLISH_NAMESPACE_DONE
sent before the answer removes every record of the namespace. Other
requests, and a request that times out, still fail. Once a new
connection takes the requests again, the lost one is retired at once, since it
delivers nothing more: each subscription continues from the next group start
on the new connection, and the objects published in between are not filled
in.

### Closing the previous connection

With NO_ERROR once the peer takes no track this side sends there and no
carried subscription has a subgroup stream open there (each sink counts the
streams it handed out until they end, `OpenSubgroups`), or when the server
closes it. A subscription with none open when it moves needs nothing more
from the previous connection, since the publisher sends every later group to
the new one too; its cutover is fixed at the group after the newest the
previous connection handed out. A group that never ends is bounded by the
5 s grace after every subscription cut over (10 s limit for a track that
delivers nothing on the new connection). A SUBSCRIBE the
previous server sent and never unsubscribed ends then with an `Unsubscribe`
event, and so does a PUBLISH the new
server rejected, which no connection takes any more.

## Alternatives

- **Keep `MigratingSession` and add an id map in wasm.** The map then exists
  in wasm and, once Rust applications want stable ids, again in `moqt`.
- **Keep the TypeScript switchover.** Migration stays in two languages.

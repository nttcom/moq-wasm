# `moqt` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
The `moqt` crate is the core implementation of Media over QUIC Transport
(draft-ietf-moq-transport-14). Every other crate in the workspace depends on it:
`relay` builds a server on top of it, `crates/wasm` runs its client session in
the browser, and the bridges/examples use its client API natively.

## Layering

```
lib.rs + subscribe.rs, publish.rs, fetch.rs,     wire.rs (raw message codecs, wasm-safe)
namespace.rs, object.rs, server.rs (public re-exports)
        │
        ▼
modules/moqt/domains      ← public API objects (Endpoint, Session, Publisher, …)
modules/moqt/runtime      ← background tasks + incoming-object dispatch
modules/moqt/control_plane← control messages, handlers, session events
modules/moqt/version      ← per-draft adapter: control channel, SETUP, control receive loop
modules/moqt/data_plane   ← object formats, stream/datagram send/receive, codecs
modules/moqt/protocol.rs  ← TransportProtocol trait + QUIC/WEBTRANSPORT/DUAL markers
        │
        ▼
modules/transport         ← transport abstraction + quinn/web-transport-quinn impls
modules/executor          ← task spawning, yielding, timeouts and the monotonic clock (tokio natively, wasm-bindgen-futures / performance.now on wasm32)
```

- The public API is grouped by MoQT operation, so the type that sends a
  request and the handler that answers it share a module:

  | Path | Contents |
  | --- | --- |
  | `moqt::` | `Endpoint`, `Session`, `SessionEvent`, `Publisher`, `Subscriber`, `Subscription`, `TrackWriter` / `TrackReader`, the transport markers, session-wide handlers (GOAWAY, MAX_REQUEST_ID, REQUESTS_BLOCKED) and errors not tied to one operation |
  | `moqt::subscribe` | SUBSCRIBE, SUBSCRIBE_UPDATE, UNSUBSCRIBE and TRACK_STATUS: options, `FilterType`, handlers |
  | `moqt::publish` | PUBLISH and PUBLISH_DONE: `PublishOption`, handlers |
  | `moqt::fetch` | FETCH and FETCH_CANCEL: options, `FetchParams`, handlers, fetch stream sender/receiver |
  | `moqt::namespace` | PUBLISH_NAMESPACE, PUBLISH_NAMESPACE_DONE, PUBLISH_NAMESPACE_CANCEL, SUBSCRIBE_NAMESPACE (and draft-18 SUBSCRIBE_TRACKS) and UNSUBSCRIBE_NAMESPACE: `PendingPublishNamespace`, `NamespaceInterest`, handlers |
  | `moqt::object` | Draft-independent object model (`Object`, `ForwardingPreference`, `SubgroupStart`, `Location`, `GroupOrder`, `ExtensionHeaders`, `ObjectStatus`) and the subgroup stream / datagram senders and receivers both SUBSCRIBE and PUBLISH deliver over |
  | `moqt::server` | `Accepting`, `Handshake`, `ServerConfig`, `TransportAddresses` |
  | `moqt::executor` | `spawn` / `JoinHandle`, the monotonic clock `now` and the wall clock `system_time`, for crates built on moqt that also run on wasm32 |

  Implementation files under `modules/` are not arranged by these modules;
  each public module only re-exports.
- The whole stack compiles for `wasm32` as well; the quinn-based transports
  and their markers (`QUIC`, `WEBTRANSPORT`, `DUAL`) are `#[cfg(not(target_arch = "wasm32"))]`
  and the `BROWSER` marker is wasm32-only. `crates/wasm` runs
  `Endpoint::<BROWSER>` and adapts its `Session` to the JavaScript callback
  API; it also uses the message codecs for the JavaScript message types.
- `wire.rs` re-exports raw control-message structs, framing helpers
  (`encode_control_message` / `take_control_message`), and data-plane object
  types for consumers that need direct wire access (the wasm bindings use
  the codecs for the JavaScript message types). The draft-14 object wire
  structs (`SubgroupHeader`, `SubgroupObjectField`, `ObjectDatagram`,
  `FetchObjectField`, …) are exported only here. Types the other crates
  reason about (`AuthorizationToken`, `fetch::FetchParams`, the typed codes)
  are exported from the operation modules instead, so a consumer that does
  not import `wire` depends on no draft-14 message layout.

## Transport layer (`modules/transport`)

`protocol.rs` defines the `TransportProtocol` trait with one associated type:

```rust
trait TransportProtocol {
    type ConnectionCreator: TransportConnectionCreator;
}
```

The creator yields `Box<dyn TransportConnection>`, whose streams are
`Box<dyn TransportSendStream>` / `Box<dyn TransportReceiveStream>`
(`BoxedConnection`, `BoxedSendStream`, `BoxedReceiveStream` in
`transport_connection.rs`), so the transport is erased as soon as a
connection exists. `try_open_bi` opens a bidirectional stream only when the
peer's stream limit allows one now: QUIC polls quinn's open once, which
takes no stream ID while pending; WebTransport keeps a pending open for the
next call, since past the stream ID it still writes the stream header; the
browser's API cannot tell, so it waits like `open_bi`.

Four zero-sized markers implement it; the first three are native-only:

| Marker | Implementation | Notes |
| --- | --- | --- |
| `QUIC` | quinn, the MoQT ALPNs (`moq-00`) | raw QUIC; used for inter-relay links and native clients |
| `WEBTRANSPORT` | web-transport-quinn, ALPN `h3` | browser-facing |
| `DUAL` | quinn endpoint dispatching on ALPN | server: accepts both `h3` (WebTransport handshake) and the MoQT ALPNs (raw QUIC) on one port. Client: one UDP socket, the URL scheme picks the ALPN per connection; both connect through `ConnectTarget::connect_quic` (IPv4 preferred, matching the IPv4 client socket), and WebTransport then runs `web_transport_quinn::Session::connect` on that connection. |
| `BROWSER` | the browser's `WebTransport` API through `web-sys` (`modules/transport/browser`) | wasm32 only, client only: `https://` URLs, certificate verification cannot be turned off, `accept` and `server` fail. Stream priority maps to `sendOrder`, RESET_STREAM codes to `WebTransportError.streamErrorCode`, datagrams are written fire-and-forget. `TransportSendStream::close` is synchronous on every transport, so it cannot wait for the peer's ACK of the FIN; here it starts `writer.close()`, which resolves only once that ACK arrives, and returns without awaiting it. `stats()` is all zeros. The `web-sys` WebTransport types are behind `--cfg web_sys_unstable_apis`, set in the root `.cargo/config.toml`. |

Only `Endpoint<T>` and its `SessionCreator<T>` are generic over
`T: TransportProtocol`, because the creator's constructors are static
(`T::ConnectionCreator::client` / `server`). Everything an endpoint produces
(`Connecting`, `Accepting`, `Handshake`, `Session`, the handlers and the data
stream types) is a plain type over the boxed transport, so one process can hold
sessions of different transports in one collection and the `DUAL` creator
simply boxes a `QUICConnection` or a `WtConnection` per connection.

Client-side helpers shared by the three creators:

- `connect_target.rs` — `ConnectTarget::parse(url)` maps the scheme to a
  transport (`moqt://` → raw QUIC, default port 4433; `https://` → WebTransport,
  default port 443) and resolves the host, preferring IPv4 because the client
  endpoint binds an IPv4 socket. `QUIC` / `WEBTRANSPORT` creators reject a URL
  for the other transport.
- `client_crypto.rs` — rustls client config (native roots, skip-verify, or a
  custom CA file), the `quinn::ClientConfig` per ALPN, and the client
  `quinn::Endpoint`.

The three creators bind server endpoints to `[::]:port` through
`bind_dual_stack` (`dual_stack_bind.rs`). On Apple targets it holds
`0.0.0.0:port` during the bind, because XNU lets `[::]:port` bind while
another socket holds `0.0.0.0:port`.

The creators do not know MoQT versions. Each takes an
`ApplicationProtocols` (`application_protocol.rs`): the ALPNs a raw QUIC
connection offers or accepts, and the WebTransport subprotocols a client
lists in WT-Available-Protocols. A WebTransport server answers with the first
of its own subprotocols the client also offered (WT-Protocol), or with none.
Every connection reports what its handshake settled on through
`TransportConnection::negotiated_protocol()`.
`BROWSER` sets `WebTransportOptions.protocols` and reads `WebTransport.protocol`
through `js_sys::Reflect`, since web-sys exposes neither; a browser without
subprotocol support reports none.

## Version adapter (`modules/moqt/version`)

Everything above the adapter states what to send and receive; the adapter
decides how a draft carries it. draft-14 and draft-18 are implemented.
`APPLICATION_PROTOCOLS` offers only draft-14 because draft-18 has not been
checked against other implementations; only tests reach draft-18, through
`version::DRAFT18_PROTOCOLS`.

A draft is added under `modules/moqt/version/<draft>` and the wire layer.
`modules/moqt/runtime/tasks` and `modules/transport` change only to route a
call through the adapter; a draft never gets a task of its own (see
*Performance invariants*).

- `Version` names the draft a session speaks.
- `ControlChannel` is an enum with one variant per draft, matched in place
  instead of a trait object: the set of drafts is closed, the match is
  checked for exhaustiveness when a draft is added, and async methods need no
  object safety or `Send` bounds that wasm32 cannot meet. It offers
  `next_request_id`, `send_request(OutgoingRequest) -> PendingResponse`,
  `respond(OutgoingResponse)` and `notify(Notification)`. The draft-14
  control message structs serve as the shared model; `draft14::ControlChannel`
  encodes them.
- `ControlReceiver` spawns the loop that turns the peer's control messages
  into `SessionEvent`s and responses; `connect` / `accept` /
  `ServerHandshake` run the draft's SETUP exchange.
- `draft14` carries everything on one bidirectional control stream: request
  ids from 0 (client) or 1 (server) in steps of two, `inflight_requests`
  pairing responses by request id, a `LateResponseAction` per request
  (how a late success is withdrawn), the 10 s response timeout,
  `ControlMessageReceiveTask` and the CLIENT_SETUP / SERVER_SETUP exchange
  (`draft14::setup`). The withdrawals:
  - SUBSCRIBE / SUBSCRIBE_NAMESPACE / PUBLISH_NAMESPACE → UNSUBSCRIBE /
    UNSUBSCRIBE_NAMESPACE / PUBLISH_NAMESPACE_DONE.
  - PUBLISH → PUBLISH_DONE (INTERNAL_ERROR, Stream Count 0: no stream opens
    before PUBLISH_OK).
  - FETCH → FETCH_CANCEL.
  - TRACK_STATUS → nothing; it creates no state.
- The data plane goes through the same seam. `SubgroupEncoder` and
  `Version::encode_datagram` / `encode_fetch_header` and `FetchEncoder`
  lay out outgoing `Object`s. `Version::accept_uni_stream` reads the header
  that tells a subgroup stream from a FETCH response and returns an
  `IncomingStream`, `SubgroupReader` / `FetchReader` turn the rest of the
  stream into `Object`s, and `Version::decode_datagram` decodes a datagram.
  `draft14::objects` holds the draft-14 side (Object ID Deltas, Subgroup ID
  forms, status codes) on top of the wire structs in `data_plane/object` and
  the decoders in `data_plane/codec`.
- `version::APPLICATION_PROTOCOLS` lists the ALPN and WebTransport subprotocol
  of every draft offered to peers; `Endpoint` hands it to its creator.
  `version::connect` / `accept` pick the `Version` from the connection's
  `negotiated_protocol()` before running that draft's SETUP exchange: ALPN
  `moq-00` (draft-14 §3.1.2) and a WebTransport session without a subprotocol
  (draft-14 defines none, §3.1.1) are draft-14; ALPN or subprotocol `moqt-18`
  is draft-18; anything else fails the session.
- `draft18` (docs/spec/draft-ietf-moq-transport-18.txt) has one
  unidirectional control stream per endpoint (SETUP, then only GOAWAY) and one
  bidirectional stream per request that carries its response, so there is no
  Request ID pairing and no late-response handling. It maps these streams onto
  the draft-14 model:
  - `ControlChannel` registers each request stream by Request ID and kind.
    `respond` writes on the peer's stream; a rejection or TRACK_STATUS_OK ends
    it with FIN. The cancelling notifications (UNSUBSCRIBE, FETCH_CANCEL,
    PUBLISH_NAMESPACE_DONE / _CANCEL, UNSUBSCRIBE_NAMESPACE) reset it, and so
    does dropping a `PendingResponse` before a success response (§3.3.2).
    When the peer ends a request, this side's half ends with FIN.
  - A FETCH's state lasts until its response stream ends (§5.2). The
    publisher ends the request stream with FIN once both its FETCH_OK and the
    FIN or reset of the response stream (`FetchDataSender::close` /
    `reset`, or its drop, which resets an unfinished response) are sent. The requester ends its half with FIN when the
    `FetchDataReceiver` reads the response stream's end or reset
    (`FetchRequestEnd`), and cancels the FETCH when the receiver is dropped
    before. A FIN from the requester does not cancel a FETCH; only a reset
    does.
  - `request_streams` reads the streams. A request the peer opens becomes the
    draft-14 event once its Request ID passes the parity and reuse checks
    (§10.1). The peer ending an open request raises the event its draft-14
    cancellation would (`Unsubscribe`, `FetchCancel`, `PublishNamespaceDone`,
    `UnsubscribeNamespace`, `PublishDone`).
  - `messages` / `parameters` encode the draft-14 structs: Message Parameters
    and Track Properties carry the former fields, absent ones take their
    draft-18 defaults, and responses take their Request ID from the stream.
    A Track Property is looked up in the mutable list and inside Immutable
    Properties (§12.7). Immutable Properties that do not parse, or that hold
    Immutable Properties, refuse the track with MALFORMED_TRACK.
  - `objects` is the draft-18 data plane. FETCH objects are delta encoded
    against the previous one, so `FetchDataSender` keeps a `FetchEncoder` and
    `accept_fetch_receiver` passes the FETCH's group order to the reader.
    A subgroup header or datagram that omits its priority takes the
    DEFAULT PUBLISHER PRIORITY its subscription's SUBSCRIBE_OK or PUBLISH
    announced (§12.4, 128 when absent): the request stream keeps the value,
    and the subscription's object sink applies it to each object it
    delivers, those buffered before the response included.
  - Messages draft-14 lacks map onto its events:
    - A `publish_namespace` matching the peer's SUBSCRIBE_NAMESPACE is sent as
      NAMESPACE on that stream and accepted at once.
    - NAMESPACE / NAMESPACE_DONE on our SUBSCRIBE_NAMESPACE raise
      `PublishNamespace` / `PublishNamespaceDone`. `PublishNamespace` gets a
      synthetic Request ID counting down from `u64::MAX`; responses to it
      send nothing.
    - SUBSCRIBE_TRACKS raises `SubscribeNameSpace` with
      `NamespaceInterest::Tracks` and its FORWARD as `forward`;
      SUBSCRIBE_NAMESPACE raises it with `Namespaces`. A SUBSCRIBE_TRACKS
      gets no NAMESPACE: a `publish_namespace` that matches only one goes as a
      PUBLISH_NAMESPACE request. PUBLISH_BLOCKED on our SUBSCRIBE_TRACKS
      raises `PublishBlocked` with that request's prefix and the Full Track
      Name, and the stream is read on (§6.1); draft-14 never raises it.
      Sending, a PUBLISH for a track the peer's SUBSCRIBE_TRACKS covers (the
      longest prefix) opens its stream with `try_open_bi`; when the limit
      allows none, PUBLISH_BLOCKED goes on that SUBSCRIBE_TRACKS stream and
      `publish` fails with `RequestFailure::PublishBlocked`. Other requests
      wait for a stream. draft-14 sends PUBLISH on the control stream.
    - `NamespaceInterest` says what `Subscriber::subscribe_namespace` asks
      for. `version::ControlChannel::namespace_requests` splits it per draft:
      draft-18 sends SUBSCRIBE_NAMESPACE for namespaces and SUBSCRIBE_TRACKS
      (`OutgoingRequest::SubscribeTracks`) for tracks, in parallel, and
      withdraws the accepted one when the other fails; draft-14 sends one
      SUBSCRIBE_NAMESPACE for any interest, and its handlers report
      `NamespacesAndTracks`. Ending either draft-18 stream raises
      `UnsubscribeNamespace` with that stream's interest;
      `unsubscribe_namespace` withdraws both.
    - A per-request GOAWAY ends the request as a peer cancellation with
      GOING_AWAY. On our SUBSCRIBE_NAMESPACE it raises
      `PublishNamespaceDone` for every namespace still announced, as the
      end of the stream does (§10.18).
    - REQUEST_UPDATE is applied as a delta to the request's current state
      (§10.9), then raised as `SubscribeUpdate`, whose `filter()` is the
      SUBSCRIPTION_FILTER it carries or `None` to keep the current one: a
      relative filter's Start Location is known only to the application, so
      it is not part of that state. Its REQUEST_OK waits for
      `SubscribeUpdateHandler::ok`, which gives the LARGEST_OBJECT (§10.2.11),
      or for the handler to be dropped, which sends none. The stream goes on
      being read meanwhile, so later updates and the peer's reset or FIN are
      seen while an answer is pending; the answers are written in the order
      the updates arrived (§10.9: exactly one per update), a later one
      waiting behind an earlier one. An update the application leaves
      unanswered for the 10 s control message timeout, counted from its
      arrival, is answered REQUEST_ERROR TIMEOUT, and a later `ok` returns
      `UpdateAnswer::Failed`: §10.9.1 then has the publisher end the
      subscription with PUBLISH_DONE UPDATE_FAILED, which only the
      application can do once it has closed the subscription's streams.
- A rule that differs per draft but applies to state only the application
  holds is asked through the handler, so the version stays inside moqt:
  `FetchHandler::check_joining` tells whether a Joining FETCH may join a
  subscription (draft-14 §9.16.2: Largest Object filter only, refused by
  closing the session; draft-18 §10.12.2: Forward State 1 only, refused with
  INVALID_RANGE), and `SubscribeUpdateHandler::may_widen` whether an
  update may move a subscription's Start Location back or its End Group out
  (draft-14 §9.10: no, refused by closing the session; draft-18 §10.9.1:
  yes).
- A FETCH parameter only one draft defines reaches the application as an
  `Option`: `Fetch::fill_timeout` / `FetchHandler::fill_timeout()` is
  draft-18's FILL_TIMEOUT (§10.2.5), always `None` on draft-14. A sender
  sets it with `FetchOption::fill_timeout`, which a draft-14 session does
  not encode.
- Error codes cross the seam as the typed enums of `error_codes.rs` and
  `constants.rs`. `draft14::error_codes` and `draft18::error_codes` hold each
  draft's values: the drafts use them to encode and decode messages, and
  `Version::termination_code` / `data_stream_reset_code` return them for
  transport closes and stream resets.

## Session establishment (`modules/moqt/domains`)

Client flow: `Endpoint` → `Connecting` (a boxed `Future`) → `Session`.
Server flow: `Endpoint` → `Accepting` (a boxed `Future`) → `Handshake` → `Session`.

- `Endpoint::create_client(&ClientConfig)` / `create_server(&ServerConfig)`
  build a `SessionCreator` around the transport's `ConnectionCreator`.
  `server::write_self_signed_cert` (feature `self-signed-cert`, off by
  default so wasm builds do not pull in `rcgen`) writes the certificate pair
  a local server or a test passes in `ServerConfig`.
- `connect(url)` returns `Connecting`, whose future performs the transport
  handshake and runs the SETUP exchange through `version::connect` (draft-14
  opens the bidirectional control stream, sends CLIENT_SETUP and awaits
  SERVER_SETUP, version `0xff00000e`). `ClientConfig.authorization_token`, when
  set, is sent as a CLIENT_SETUP AUTHORIZATION TOKEN parameter (Alias Type
  `USE_VALUE`, Token Type `0`, UTF-8 value). On success a `Session` is
  created.
- `refuse_new_connections()` makes every later connection attempt fail at
  the transport level (QUIC CONNECTION_REFUSED, HTTP 503 for WebTransport)
  while `accept()` keeps being polled; established sessions are unaffected.
  A closing server uses it so that clients learn at once instead of waiting
  for their handshake to time out.
- `accept()` returns `Accepting`, or `AcceptError::EndpointClosing` once the
  endpoint is closed, which every transport reports the same way. The future
  of `Accepting` performs the transport handshake and `version::accept`,
  which for draft-14 accepts the control stream and stops after CLIENT_SETUP
  is received. It resolves to a
  `Handshake` that exposes the CLIENT_SETUP's AUTHORIZATION TOKEN parameters
  (`authorization_tokens()`) so the application can decide before
  SERVER_SETUP is sent:
  - `Handshake::accept()` answers the setup (SERVER_SETUP for draft-14) and
    creates the `Session`.
  - `Handshake::reject(code, reason)` closes the transport with the given
    termination code without sending SERVER_SETUP.

### `Session` and its background tasks

`Session::new` starts the `ConnectionTasks` of its connection
(`runtime/tasks/connection_tasks.rs`): five tasks bound to that `Connection`,
all named and all aborted when `ConnectionTasks` is dropped:

| Task | Role |
| --- | --- |
| control receiver (`ControlReceiver::spawn`; draft-14: `ControlMessageReceiveTask`) | reads the control stream, decodes messages, routes them (see below); the handlers it creates answer on its connection. Holds only `Weak` references to the `SessionContext` and the `Connection`, so it cannot keep the session alive. |
| `UniStreamReceiveTask` | accepts incoming unidirectional streams and reads their header through `Version::accept_uni_stream`: a subgroup stream goes to `SubscriptionNotifier`, a FETCH response to `FetchNotifier`. Headers are read concurrently, so a stream whose header has not arrived does not block the streams accepted after it. |
| `DatagramReceiveTask` | receives datagrams, decodes them through `Version::decode_datagram`, dispatches the `Object` via `SubscriptionNotifier`. |
| `DisconnectWatchTask` | awaits transport close, then emits `SessionEvent::Disconnected`. This is also how a close this side requested with any code other than PROTOCOL_VIOLATION is reported; `close_with_error(ProtocolViolation, ..)` emits `SessionEvent::ProtocolViolation` at once instead. |
| `PublishDoneExpiryTask` | receives the Request ID of every PUBLISH_DONE from `ControlMessageReceiveTask` and cancels that subscription of its connection 10 s later (see *Key invariants*). Holds only `Weak` references. |

A client session with `ClientConfig::follow_go_away` (the default) follows
the server's GOAWAY by itself (draft-14 §3.5; see
[session-migration.md](session-migration.md)). Its connections send their
events to a `MigrationTask` (`domains/session/migration_task.rs`) instead of
the application: it forwards them with logical ids, and on GOAWAY it opens
the next connection through the endpoint's `SessionCreator` (`Reconnect`),
sends every namespace subscription (with its `NamespaceInterest`),
PUBLISH_NAMESPACE, PUBLISH and SUBSCRIBE made on the session again
(`NextGroupStart`, the last Forward state; a rejected namespace or SUBSCRIBE
is repeated every 500 ms for up to 10 s; the connection keeps the Start
Location the new SUBSCRIBE_OK resolves, which `update_forward` and
`end_before_group` repeat there),
makes it the current connection, and closes the previous one with NO_ERROR
once the server takes no track this side sends there any more and no
carried subscription has a subgroup stream open there (each sink counts the
streams it handed out until they end, `OpenSubgroups`) or still owes a group
before the start the new SUBSCRIBE_OK resolved, at the latest 5 s
after every carried subscription cut over (at most 10 s after the move), or
when the server closes it; a SUBSCRIBE still served there then ends with an
`Unsubscribe` event. A track PUBLISHed on the session goes to every
connection it is on: each subgroup stream the `StreamDataSenderFactory`
opens, and each datagram, is sent on all of them, the new connection from
the next subgroup on (draft-22 §7.6.1). An UNSUBSCRIBE that drops a
publication from one connection while it is still on another is not
returned; requests the peer sends, on any connection, carry logical ids. A
FETCH response stream a migration cuts off is FETCHed again on the current
connection from the first object not delivered up to the FETCH_OK's End
Location, and the `FetchDataReceiver` reads on from it; a descending FETCH
is not sent again. The FETCH sent again carries the FILL_TIMEOUT left of the
first one (draft-18 §10.2.5), counted from when the first was sent and 0
once it ran out. While both
connections deliver a subscription, the sinks of both share a `GroupCutover`
(see *Key invariants*). A PUBLISH_DONE the previous connection sends for a
carried subscription is dropped; GOAWAY is never returned; `Disconnected`
comes once, when the current connection closed and no migration replaced
it, and `Session::closed()` resolves then. A session without
`follow_go_away`, and every server session, reads its one connection as
before and returns GOAWAY as an event.

`Session::publisher()` / `subscriber()` return lightweight `Publisher` /
`Subscriber` facades sharing the same `Arc<SessionContext>`. Operations on an
established request take its handle, not its request id:
`Subscriber::unsubscribe` / `subscribe_update` / `update_forward` / `end_before_group` /
`fetch_relative_joining` take the `Subscription`, `Subscriber::fetch_cancel`
the `FetchHandle`, and `Publisher::publish_done` the `Subscription` it
publishes on. The request id stays readable as an identifier, but how a
request is cancelled or updated on the wire is left to moqt. Application
code consumes inbound control messages through
`Session::receive_event() -> SessionEvent`. `Session::closed()` awaits the
transport close itself (the signal `DisconnectWatchTask` turns into
`SessionEvent::Disconnected`) and returns the peer's termination code and
reason as `TransportClose`; it reads no session events, so a task can await it
beside `receive_event()`. `Session::url_scheme()` is `moqt` for raw QUIC and
`https` for WebTransport, so a server sending GOAWAY can name the new session
URI in the scheme the client connected with (§9.4).
The session's `Connection` knows whether it is the server side and whether a
GOAWAY was already received: a second GOAWAY, or one carrying a New Session
URI at a server, closes the session with PROTOCOL_VIOLATION before any event
is emitted (§9.4 MUSTs).
`Publisher::begin_publish_namespace` returns once PUBLISH_NAMESPACE is written,
with a `PendingPublishNamespace` whose `accepted()` awaits the reply, so a
caller fanning out to many sessions can keep the writes ordered without
waiting on each peer.

### `TrackWriter` / `TrackReader` — track-level object I/O

Applications that only need "one payload per object, one subgroup per group"
use these facades instead of the data-plane factories directly:

- `TrackWriter` wraps `StreamDataSenderFactory`. `start_group()` closes
  the open group (sends an `EndOfGroup` status object, then FIN) and opens the
  next one; `start_group_at(id)` opens a group with a chosen id instead, so
  the tracks of a switching set can start their groups at the same ids, and
  announces any ids it skipped on the group's first object with the Prior
  Group ID Gap extension header (a writer that has not opened a group yet may
  start anywhere). `write()` appends a payload object with optional immutable
  extension headers and `write_with_extension_headers()` with arbitrary ones,
  which is how LOC headers are attached. Group ids run from the
  `first_group_id` given at construction unless chosen, `next_group_id()`
  exposes where numbering continues, object ids run from 0 within each group;
  publisher priority is fixed at 128 and the subgroup id is omitted
  (`SubgroupId::None`).
- `TrackReader` wraps `StreamDataReceiverFactory` and yields
  `TrackObject { group_id, subgroup_id, object_id, extension_headers, payload }`
  in arrival order. Subgroup streams are read **concurrently**: an accept task
  takes every new stream from the factory and spawns one reader task per
  stream; readers feed a bounded channel (64 objects) so a slow consumer
  applies QUIC flow control to the publisher instead of buffering without
  bound. Ordering is therefore only guaranteed within a subgroup (draft-14
  §10.4); consumers that need cross-subgroup order sort by the ids themselves.
  It reads the resolved objects `StreamDataReceiver` returns (below);
  status objects are skipped, a failure inside one stream is returned as `Err`
  while the other streams keep being read, and `Ok(None)` means the factory
  has no more streams.

### `Origin` — the tracks this endpoint originates

`Origin` (`domains/origin.rs`) is the opt-in bookkeeping of an original
publisher: a `Clone` handle over a `tokio::sync::Mutex` of per-track state,
shared between the application's session event loop and its media writers.
Relays do not use it; they
forward from their own cache through the raw stream senders.

- Per track (`ensure_track(namespace, name, first_group_id, retention)`):
  `ObjectNumbering` (`GroupBoundary::{Within, Next, At}` → `Placement`),
  an `ObjectCache` with `Retention::For(duration)` or `Retention::NewestOnly`,
  and one outlet per subscription to
  the track (a SUBSCRIBE served or a PUBLISH sent): its publication state and
  its optional `TrackWriter`. An outlet is keyed by the session, identified
  by its `SessionContext`, which the SUBSCRIBE, UNSUBSCRIBE, FETCH and
  FETCH_CANCEL handlers carry, and the logical request id, so the SUBSCRIBEs
  an old and a new server send during a migration get one outlet each. The
  caller chooses the retention and the first group id.
- `write(namespace, name, OutgoingObject) -> WriteOutcome` numbers and
  caches the object first, whether or not the track is published, so the
  cache and the live streams always agree on ids, then sends it to every
  outlet at once. Each writer is taken out of its slot for the duration of
  the awaited send and put back only if nothing replaced or released it
  meanwhile. A send that fails on one outlet is returned only when no outlet
  took the object. `Sent(placement)` and
  `Stopped(placement)` (STOP_SENDING by the peer; the track resumes at the
  next group) carry the placement so the caller can account the stream;
  `NotSent` covers a `Within` object with no open group, a track without a
  writer, and writers that start after the object or wait for the next group
  after a stopped stream.
  `write_group` sends one object as a finished group (the MSF catalog layout).
- `publish(session, …)` sends PUBLISH on that session and writes from the
  next group; it is a no-op while the session takes the track and after it
  released a PUBLISH, but publishes a track again whose SUBSCRIBE the peer
  released; `accept_subscribe`
  answers a SUBSCRIBE the application decided to accept (Content Exists is the
  largest cached location, the last one FETCH can serve) and writes from its filter's Start Location
  (draft-14 §9.7), computed from the largest object written so far. Objects
  before the Start Location are skipped; when the first object at or after it
  does not start a group, the writer opens that group on a new stream starting
  at that object, so Largest Object takes the rest of the open group;
  `release(&handler)`
  drops the session's writer on UNSUBSCRIBE, resetting its open group with
  CANCELLED rather than finishing it (draft-14 §10.4.3), and returns the
  track so the caller can account it; `is_serving(session)` tells whether
  that session still takes any track; `serve_fetch` answers a Standalone FETCH from the cache on its
  own stream (`FetchServeTask`, one per request, aborted by `cancel_fetch` and
  when the `Origin` is dropped; joining FETCH is NOT_SUPPORTED);
  `answer_track_status` fills TRACK_STATUS_OK like a SUBSCRIBE_OK.
- `handle(event)` is the session event loop's shortcut to all of the above:
  it accepts a SUBSCRIBE to a registered track, releases on UNSUBSCRIBE,
  serves FETCH, aborts on FETCH_CANCEL and answers TRACK_STATUS, and hands
  back every other event, and a SUBSCRIBE or UNSUBSCRIBE for a track or
  request it does not know, to the caller.
- Left to the application: whether to accept a SUBSCRIBE for a track it
  has not registered yet (`handle` returns it), namespace
  announcements, when to PUBLISH, retention and first group ids, and
  disconnect handling. The FETCH End Location arithmetic is the free function
  `resolve_fetch_range` (`domains/fetch_range.rs`), shared with the wasm
  bindings.

### `SessionContext` and `Connection` — shared session state

`Connection` (`domains/connection.rs`) holds what draft-14 scopes to one
transport connection:

- `transport` — the QUIC or WebTransport connection.
- `control: ControlChannel` — the version adapter above; for draft-14 it
  holds the request id counter and `inflight_requests`.
- `track_alias: AtomicU64` — the track aliases this side assigns, shared by
  `SubscribeHandler::allocate_track_alias` and `Publisher::publish`. Both
  hand the application the logical alias (below); SUBSCRIBE_OK and PUBLISH
  carry the wire part.
- whether it is the server side and whether a GOAWAY arrived.
- `generation` (0 for the first connection) and the ids of requests carried
  over from an earlier connection. A logical id, the id the application
  sees, is `generation << 56 | wire id`, so the first connection's ids are
  their own logical ids and no two connections' ids meet in the session's
  maps.
- `object_sinks: HashMap<track_alias, ObjectSink>` — the objects arriving on
  this connection, keyed by wire alias; a sink sets the logical alias on
  every stream it hands out; see buffering invariant below.
- `fetch_notification_map` — keyed by request id; `FetchNotifier` removes the
  sender when the FETCH stream arrives, since a FETCH is answered on a single
  stream (draft-14 §9.16.3).

`SessionContext` holds the rest of the cross-task state, the current
`Connection` and, during a migration, the previous one; `connection()` is the
connection requests are sent on, and the methods that touch per-connection
state name the connection:

- `event_sender`.
- `receiver_map` and `fetch_receiver_map` — the receivers the application
  takes per logical track alias and per logical FETCH request id.
  Registering a FETCH returns a `PendingFetch` drop-guard that removes both
  FETCH entries unless FETCH_OK arrived.
- `carried` — for a session that follows GOAWAY, the namespace (with their
  `NamespaceInterest`) and track subscriptions and the published namespaces a
  migration sends again.
- `publications` — for a session that follows GOAWAY, the tracks PUBLISHed
  on it with every connection each is on (`SendTargets::Publication`); a
  track served for a SUBSCRIBE goes to the connection the SUBSCRIBE arrived
  on (`SendTargets::Connection`). It is locked while one of them is
  sent and while a migration sends them all, so none is missed.

## Control plane (`modules/moqt/control_plane`)

- `control_messages/messages/*` — one struct per draft-14 control message with
  `encode`/`decode`. Framing (varint type + **16-bit fixed length**, a draft-14
  change) lives in `wire.rs`.
- `handler/*` — received-message facades handed to the application inside
  `SessionEvent` (e.g. `SubscribeHandler::ok(...)`, `::error(...)`). Their
  `ResponseGuard` keeps the `Connection` the request arrived on, so the answer
  goes there and responding does not require the `Session`.
  Namespace-carrying handlers expose the track namespace both as the wire
  tuple (`track_namespace_tuple`) and as the `/`-joined string
  (`track_namespace`); the tuple is authoritative because a tuple element may
  itself contain `/`.
- MAX_CACHE_DURATION (draft-14 §9.2.1.3, milliseconds) is read from received
  messages and set on outgoing ones, but never enforced here; caching
  endpoints such as the relay apply it.
  Received values are on `Subscription::max_cache_duration()` (from
  SUBSCRIBE_OK or PUBLISH), `PublishHandler.max_cache_duration` and
  `FetchHandle.max_cache_duration`; values to send go in
  `SubscribeHandler.max_cache_duration`, `FetchHandler.max_cache_duration` and
  `PublishOption.max_cache_duration`.
- `enums.rs` — `SessionEvent` (inbound requests + `Disconnected` /
  `ProtocolViolation`) and the crate-private `ResponseMessage`.
- `constants.rs` — protocol version and `TerminationErrorCode` (draft-14
  §13.1.1).
- `error_codes.rs` — typed codes the public API uses instead of integers. They
  carry no values; the version adapter maps them (`draft14::error_codes`):
  - `RequestErrorCode` (handler `error()`, `RequestFailure::Rejected`) is one
    semantic enum; draft-14 numbers codes per `*_ERROR` message, so the wire
    value is looked up per `RequestKind`.
  - `PublishDoneCode` (`Publisher::publish_done`,
    `PublishDoneHandler::status_code`) and `DataStreamResetCode` (the data
    stream senders' `reset()`).
  - A code the message does not define is sent as INTERNAL_ERROR; an unknown
    received value (request error or PUBLISH_DONE status) reads as
    INTERNAL_ERROR.
  - Integers remain only in the `moqt::wire` structs. `moqt::wire` also
    exports `RequestKind` and the draft-14 conversions
    (`request_error_code_to_wire` / `request_error_code_from_wire`,
    `publish_done_code_from_wire`) for the wasm bindings, whose JavaScript
    API carries draft-14 numbers.
  - `ResponseGuard` is the one place that encodes and sends a `*_ERROR`, for
    both the handlers' `error()` and the drop-time NOT_SUPPORTED reply.

The draft-14 `ControlMessageReceiveTask` splits every decoded message into
one of three paths:

1. **Requests** (SUBSCRIBE, PUBLISH, FETCH, namespace messages, …) become
   `SessionEvent` variants delivered to `Session::receive_event()`.
2. **Responses** (`*_OK` / `*_ERROR`) are matched against `inflight_requests` by
   request id and complete the pending `oneshot`. The waiting side holds a
   `PendingResponse` whose drop marks the entry abandoned, so a late response
   runs its `LateResponseAction` instead of closing the session. When the
   control stream closes, every entry is failed and later registrations fail
   at once.
3. **Protocol violations** (a malformed payload, a SETUP after the handshake,
   a response to a request kind this crate never sends) close the session and
   stop the task.

Messages `Session` decodes but does not act on (GOAWAY, unless the session
follows it, MAX_REQUEST_ID,
REQUESTS_BLOCKED, SUBSCRIBE_UPDATE, PUBLISH_DONE, FETCH_CANCEL,
PUBLISH_NAMESPACE_CANCEL) still take path 1, so the application sees them
instead of the session dying.

TRACK_STATUS is a request whose format is that of SUBSCRIBE (§9.20).
`Subscriber::track_status(namespace, name, authorization_tokens)` sends it and
returns the TRACK_STATUS_OK it awaits on path 2 (TRACK_STATUS_ERROR becomes a
`RequestError`); a late TRACK_STATUS_OK
is discarded because the request creates no state. The receiving side gets
`SessionEvent::TrackStatus(TrackStatusHandler)` with the request's
AUTHORIZATION TOKEN parameters exposed. `TrackStatusHandler::ok(expires,
content_exists)` fills TRACK_STATUS_OK the way `SubscribeHandler` fills a
SUBSCRIBE_OK (Group Order resolved by `GroupOrder::delivered`) with Track
Alias 0 (§9.21); the caller supplies the track's status, which the relay
reports as Content Exists false when TRACK_STATUS only carries an
authorization token refresh. The handler carries a
`ResponseGuard`, so an application that ignores the event answers
TRACK_STATUS_ERROR NOT_SUPPORTED automatically.

## Data plane (`modules/moqt/data_plane`)

- `object/canonical_object.rs` — the draft-independent data model the public
  stream API speaks: `Object` (location, `ForwardingPreference`, publisher
  priority, status, extension headers, payload) and `SubgroupStart` (what a
  subgroup header announces: group, subgroup, priority, whether the subgroup
  ends its group). A zero-length object carries its meaning in the status.
- `object/*` (the rest) — draft-14 wire formats: `SubgroupHeader` (types
  `0x10..=0x1D` encoding subgroup-id mode / extensions / end-of-group),
  `SubgroupObject`, `ObjectDatagram`, `FetchObject`, `ExtensionHeaders`,
  `ObjectStatus`.
  `ExtensionHeaders` keeps every Object Extension Header as an ordered list of
  `KeyValuePair`s and re-encodes them unchanged (accessors expose the header
  types the transport uses); this satisfies the draft-14 §10.2.1.2 requirement
  to forward unsupported extension headers unmodified, and lets higher layers
  (e.g. LOC) define their own header types without changing the transport.
- `codec/*` — incremental decoders (`ControlMessageDecoder`,
  `UniStreamDecoder`, `SubgroupDecoder`, `FetchDecoder`) over a shared
  `tokio_reader`.
- `stream/*` — send/receive wrappers:
  - `StreamDataSenderFactory::next()` opens one uni stream per subgroup and
    returns a `StreamDataSender` for the session's `Version`.
  - `StreamDataSender` uses a **typestate** (`Uninitialized` → `start()`
    → `HeaderSent`) so "header before objects" is enforced at compile time.
    For draft-14, `start(SubgroupStart)` always declares extensions present
    and omits the Subgroup ID field for subgroup 0; `send_object(&Object)`
    computes the Object ID Delta from the previous object and rejects ids that
    do not increase. `TrackWriter` sends through the same two calls.
    `begin_object(&Object, payload_len)` sends an object up to its payload
    and `send_payload(&Bytes)` the payload in pieces, so a relay can forward
    an object before its last byte arrived; the sender rejects a piece past
    the announced length, and `send_object` / `begin_object` / `close` while
    a payload is incomplete. Raw header and object-field sends exist only in
    tests of how a receiver resolves each header type and delta.
  - `StreamDataReceiver::receive()` returns `Subgroup::Header(SubgroupStart)`
    and then `Subgroup::Object(Object)` with absolute ids: header types
    without a Subgroup ID field mean subgroup 0, and the "first object id"
    types (0x12/0x13/0x1A/0x1B) are returned only once the first object
    started to arrive (a stream that ends before it yields no header). An
    Object Status draft-14 does not define is
    `StreamReceiveError::ProtocolViolation`.
  - `StreamDataReceiver::into_parts()` returns a `SubgroupPartReceiver`
    whose `receive()` hands over `SubgroupPart`s: the header, whole objects
    whose payload arrived with the fields before it, and otherwise
    `ObjectStart { object, payload_len }` followed by `Payload { data, last }`
    pieces as they arrive. `SubgroupDecoder` produces both shapes: it splits
    an object only when its payload is not buffered with its head, and
    `StreamDataReceiver::receive()` puts the pieces back together. Payload
    pieces share the read buffer (no copy); a whole object's payload is
    copied, because a cache may keep it long after the buffer would
    otherwise be freed. A stream that ends inside a payload is a decode
    error.
  - `DatagramSender::send_object(&Object)` / `DatagramReceiver::receive()
    -> Object` do the same for datagrams (`ObjectDatagram::from_object` /
    `into_object`): an object id is always sent explicitly, a zero-length
    object is sent as its status (§10.2.1.1), and an object with a payload is
    always `Normal`: an End of Group type sets `Object::ends_group` instead of
    the status, and only a datagram with a payload sends it back. FETCH cannot
    carry it, so a fetched copy of that object has `ends_group` false.
  - `FetchDataSender::send_object(&Object)` / `FetchDataReceiver::receive()
    -> Fetch::{Object(Object), UnknownRange { last }, End}` do the same for
    FETCH responses (`FetchObjectField::from_object` / `into_object`): a
    datagram object is sent with its Object ID as the Subgroup ID (§10.4.4)
    and a zero-length object as its status. The FETCH_HEADER is consumed by
    moqt and not returned. `UnknownRange` is draft-18's End of Unknown Range
    (§11.4.4.2): the objects through `last` were not sent and may exist, so
    `End` after it does not mean the range is complete. draft-14 never
    yields it; draft-18's End of Non-Existent Range yields nothing.
    The sending side is `FetchDataSender::report_unknown_range(last) ->
    UnknownRange`: draft-18 writes the marker and the response goes on
    (`Marked`). A Group ID Delta always moves to another group (§11.4.4.1),
    so a range ending in the group of the object or marker before it cannot
    be written; there, and on draft-14, which has no marker, the response is
    reset with UNKNOWN_OBJECT_STATUS (draft-18 §10.12; draft-14's wire value
    is INTERNAL_ERROR) and has ended (`Reset`).
    `FetchHandler::marks_unknown_ranges()` tells beforehand whether the
    response can do the former, so a relay can choose how long to wait for
    objects without knowing the draft.
  - `StreamDataSender::set_priority(StreamPriority)` (`Uninitialized` only)
    maps the draft-14 §7.2 scheduling inputs onto the transport's per-stream
    send priority (`TransportSendStream::set_priority(i32)`, quinn semantics:
    higher first). `stream_priority.rs` packs the four §7.2 rules most
    significant first — subscriber priority, publisher priority, group rank,
    subgroup id — into a negative `i32`, so the control stream (transport
    default 0) always outranks data streams. The group rank is the caller's
    `group_sequence` (how many groups the subscription opened before this one)
    modulo 2^12, reversed for Descending group order; the subgroup id
    saturates at 7. These values never reach the wire: MoQT headers carry the
    original priority numbers.
  - `StreamDataReceiverFactory` / `StreamDataReceiver`, `DatagramSender` /
    `DatagramReceiver`, `FetchDataSender` / `FetchDataReceiver` mirror this on
    the other side.

## Runtime dispatch (`modules/moqt/runtime/dispatch`)

- `SubscriptionNotifier` routes incoming objects by track alias into
  `Connection::object_sinks`.
- `FetchNotifier` routes FETCH streams by request id as a
  `FetchDataReceiver`.
- `IncomingObject` is the per-track-alias envelope (`Subgroup` holding a
  `StreamDataReceiver` / `Datagram` holding an `Object`); FETCH streams never
  enter the per-track channel.

## Key invariants

- **A carried subscription cuts over at a group boundary.** The first object
  the new connection delivers fixes the cutover group: the group after the
  newest one the previous connection handed out, or that object's group when
  it is newer and starts the group. The previous connection's sink keeps
  handing out the groups before it and drops the rest; the new one drops the
  groups before it. A subgroup stream carries one group and counts as
  starting it, since the subscription sent again starts at the next group
  start. The publisher sends to both servers at once, so the cutover does not
  wait for the new connection to deliver a group first. Once the previous
  connection has no subgroup stream of the subscription open, a cutover still
  waiting is fixed at the group after the newest it handed out, so no group
  is delivered twice when the previous connection closes.

- **Buffer-before-subscribe**: objects can arrive before the application
  registers a receiver (data races SUBSCRIBE_OK). `object_sinks` buffers up to
  256 objects per track alias (`MAX_PENDING_OBJECTS_PER_TRACK_ALIAS` in
  `session_context.rs`), dropping the oldest on overflow.
  `register_data_receiver` drains the buffer into the new channel atomically
  under the sinks lock.
- **Cancelled subscriptions discard late objects** (draft-14 §10.1):
  `Subscriber::unsubscribe`, the UNSUBSCRIBE sent for a late SUBSCRIBE_OK and
  `PublishHandler::error` (a PUBLISH with Forward=1 sends objects before its
  answer, §9.13) turn the track alias's sink into `Cancelled`, which drops
  the buffered objects, the channel sender and a receiver not yet taken by
  `accept_data_receiver`. Objects still in flight are then dropped on arrival
  instead of being buffered as an unknown alias until the session ends. The
  `Cancelled` marker stays until a new subscription reuses the alias; it is
  not released on PUBLISH_DONE because late streams may follow it (§9.12).
- **PUBLISH_DONE ends the subscription after a grace period** (draft-14
  §9.12): the event still reaches the application, and 10 s later
  `PublishDoneExpiryTask` cancels the subscription as an unsubscribe would,
  without sending UNSUBSCRIBE. Streams that arrive within the 10 s are
  delivered; afterwards the data receiver ends, and `TrackReader` returns
  `Ok(None)` once the streams it already accepted are drained. Stream Count is
  not used to end it earlier, and no STOP_SENDING is sent.
- **Duplicate track alias**: if SUBSCRIBE_OK carries a track alias that already
  has a registered receiver, the subscriber closes the session with
  `DuplicateTrackAlias` (draft-14 §9.8). A cancelled subscription's alias does
  not count as in use.
- **Unknown response request id**: a response that matches no `inflight_requests`
  entry is a protocol violation; the session is closed with
  `ProtocolViolation`.
- **SUBSCRIBE_UPDATE for a Request ID that never existed** (draft-14 §9.10):
  the draft-14 `ControlChannel` keeps the next Request ID of either side, and
  a Subscription Request ID at or above the next one of its parity closes the
  session with `ProtocolViolation`. An update of a subscription that has
  ended still reaches the application.
- **Control response timeout**: `PendingResponse::response` bounds every
  request/response wait to 10 s; a timeout fails only that request with
  `RequestFailure::Timeout` and keeps the session open (draft-14 §12.2).
  Every request that waits for a response returns `RequestFailure`, so
  callers tell a peer rejection, a timeout and a closed session apart by
  matching instead of downcasting.
- **One forwarding preference per track alias** (draft-14 §2.5, §10): the
  first object decides whether `accept_data_receiver` returns a stream or a
  datagram receiver. An object of the other kind later makes that receiver
  return `MalformedTrackError`, which `TrackReader` passes on. moqt does not
  UNSUBSCRIBE on its own: the caller does, because a relay must also end
  its downstream subscriptions.
- **Header-first subgroup streams**: enforced by the sender typestate; on the
  receive side a uni stream whose first frame is not a header is rejected.
- **Priority before data**: a stream's transport priority is settable only
  before its header, because the transport applies a changed priority to a
  stream with queued data only after that stream's next frame.
- **Session teardown**: dropping `Session` aborts all four background tasks;
  the control task's `Weak` reference guarantees it never keeps the context
  alive. The transport closes (code 0) once the last handle to the context
  (`Session`, `Publisher`, `Subscriber`, stream writers) is gone, but the
  CONNECTION_CLOSE is sent by quinn's background driver: a process must
  await `Endpoint::wait_idle()` before exiting, or the peer only notices
  through its idle timeout.
- **No direct runtime calls in the session stack**: `modules/moqt` spawns,
  yields, bounds waits and reads the clock only through `modules/executor`
  (`spawn`, `try_spawn`, `yield_now`, `timeout`, `now`, `system_time`); `tokio::sync`
  channels and locks are used directly. `modules/transport` is the only other
  place that touches tokio I/O. `executor/native.rs` is the tokio
  implementation (`now` is the elapsed time since a process-wide `Instant`),
  `executor/browser.rs` the `spawn_local` one (`now` reads
  `performance.now()`; `std::time::Instant` panics on wasm32); the executor also defines
  `MaybeSend` / `MaybeSync` / `BoxFuture`, which are `Send` / `Sync` bounds
  natively and empty on wasm32 because JavaScript values cannot cross
  threads. The transport traits and `async_trait` attributes use them.

## Performance invariants

Two latency fixes live in version-independent layers and must hold for every
draft:

- **A stream whose header has not arrived does not hold back later streams**
  (`UniStreamReceiveTask`). Headers are read concurrently; a draft only
  parses them, in `Version::accept_uni_stream`. Guarded by
  `uni_stream_receive_task::tests::a_stream_without_its_header_does_not_hold_back_later_streams`,
  which writes draft-14 header bytes through `open_subgroup`, so it covers
  draft-14 only, and by the meeting E2E below, which bounds the longest gap
  between consecutive audio or video arrivals across audio group boundaries.
- **Closing a send stream does not wait for the peer to acknowledge the FIN**
  (`TransportSendStream::close`). quinn and WebTransport call `finish()`; the
  browser transport does not await `writer.close()`. Awaiting the ACK stalls
  every send queued behind a stream close. Guarded by the meeting E2E
  `rotating the audio group neither stalls the sender nor gaps the receiver`
  (`examples/browser/tests/meeting-e2e.spec.ts`), which bounds how long the
  publisher's audio EndOfGroup and stream close take. On Linux the runner
  (`tests/browser-e2e/run-meeting-e2e.mjs`) delays every relay reply with
  `tc netem`, so a close that awaits the ACK exceeds the bound even though CI
  runs the relays on the same host. The measurement lives in
  `examples/browser/utils/media/audioTransport.ts`, which every draft shares.

## Testing conventions
Unit tests live in `#[cfg(test)] mod tests` inside the module under test
(Arrange/Act/Assert, see `wire.rs` framing round-trip tests). Cross-crate
behaviour is covered by the workspace-level E2E suites under `tests/`.

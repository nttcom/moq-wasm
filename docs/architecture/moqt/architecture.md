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
  | `moqt::namespace` | PUBLISH_NAMESPACE, PUBLISH_NAMESPACE_DONE, PUBLISH_NAMESPACE_CANCEL, SUBSCRIBE_NAMESPACE and UNSUBSCRIBE_NAMESPACE: `PendingPublishNamespace`, handlers |
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
connection exists.

Four zero-sized markers implement it; the first three are native-only:

| Marker | Implementation | Notes |
| --- | --- | --- |
| `QUIC` | quinn, the MoQT ALPNs (`moq-00`) | raw QUIC; used for inter-relay links and native clients |
| `WEBTRANSPORT` | web-transport-quinn, ALPN `h3` | browser-facing |
| `DUAL` | quinn endpoint dispatching on ALPN | server: accepts both `h3` (WebTransport handshake) and the MoQT ALPNs (raw QUIC) on one port. Client: one UDP socket, the URL scheme picks the ALPN per connection; both connect through `ConnectTarget::connect_quic` (IPv4 preferred, matching the IPv4 client socket), and WebTransport then runs `web_transport_quinn::Session::connect` on that connection. |
| `BROWSER` | the browser's `WebTransport` API through `web-sys` (`modules/transport/browser`) | wasm32 only, client only: `https://` URLs, certificate verification cannot be turned off, `accept` and `server` fail. Stream priority maps to `sendOrder`, RESET_STREAM codes to `WebTransportError.streamErrorCode`, datagrams are written fire-and-forget. `stats()` is all zeros. The `web-sys` WebTransport types are behind `--cfg web_sys_unstable_apis`, set in the root `.cargo/config.toml`. |

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
decides how a draft carries it. Only draft-14 is implemented: the adapter
exists ahead of draft-18 support, as a deliberate exception to the
coding-style rule that an abstraction waits for its second concrete use.

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
  `Version::encode_datagram` / `encode_fetch_header` / `encode_fetch_object`
  lay out outgoing `Object`s. `Version::accept_uni_stream` reads the header
  that tells a subgroup stream from a FETCH response and returns an
  `IncomingStream`, `SubgroupReader` / `FetchReader` turn the rest of the
  stream into `Object`s, and `Version::decode_datagram` decodes a datagram.
  `draft14::objects` holds the draft-14 side (Object ID Deltas, Subgroup ID
  forms, status codes) on top of the wire structs in `data_plane/object` and
  the decoders in `data_plane/codec`.
- `version::APPLICATION_PROTOCOLS` lists the ALPN and WebTransport subprotocol
  of every supported draft; `Endpoint` hands it to its creator.
  `version::connect` / `accept` pick the `Version` from the connection's
  `negotiated_protocol()` before running that draft's SETUP exchange: ALPN
  `moq-00` (draft-14 §3.1.2) and a WebTransport session without a subprotocol
  (draft-14 defines none, §3.1.1) are draft-14; anything else fails the
  session.
- Error codes cross the seam as the typed enums of `error_codes.rs` and
  `constants.rs`. `draft14::error_codes` holds their draft-14 values: `draft14`
  uses them to encode and decode messages, and `Version::termination_code` /
  `data_stream_reset_code` return them for transport closes and stream resets.

## Session establishment (`modules/moqt/domains`)

Client flow: `Endpoint` → `Connecting` (a boxed `Future`) → `Session`.
Server flow: `Endpoint` → `Accepting` (a boxed `Future`) → `Handshake` → `Session`.

- `Endpoint::create_client(&ClientConfig)` / `create_server(&ServerConfig)`
  build a `SessionCreator` around the transport's `ConnectionCreator`.
- `connect(url)` returns `Connecting`, whose future performs the transport
  handshake and runs the SETUP exchange through `version::connect` (draft-14
  opens the bidirectional control stream, sends CLIENT_SETUP and awaits
  SERVER_SETUP, version `0xff00000e`). `ClientConfig.authorization_token`, when
  set, is sent as a CLIENT_SETUP AUTHORIZATION TOKEN parameter (Alias Type
  `USE_VALUE`, Token Type `0`, UTF-8 value). On success a `Session` is
  created.
- `accept()` returns `Accepting`, whose future performs the transport
  handshake and `version::accept`, which for draft-14 accepts the control
  stream and stops after CLIENT_SETUP is received. It resolves to a
  `Handshake` that exposes the CLIENT_SETUP's AUTHORIZATION TOKEN parameters
  (`authorization_tokens()`) so the application can decide before
  SERVER_SETUP is sent:
  - `Handshake::accept()` answers the setup (SERVER_SETUP for draft-14) and
    creates the `Session`.
  - `Handshake::reject(code, reason)` closes the transport with the given
    termination code without sending SERVER_SETUP.

### `Session` and its background tasks

`Session::new` spawns five tasks (all named, all aborted in `Drop`):

| Task | Role |
| --- | --- |
| control receiver (`ControlReceiver::spawn`; draft-14: `ControlMessageReceiveTask`) | reads the control stream, decodes messages, routes them (see below). Holds only a `Weak<SessionContext>` so it cannot keep the session alive. |
| `UniStreamReceiveTask` | accepts incoming unidirectional streams and reads their header through `Version::accept_uni_stream`: a subgroup stream goes to `SubscriptionNotifier`, a FETCH response to `FetchNotifier`. |
| `DatagramReceiveTask` | receives datagrams, decodes them through `Version::decode_datagram`, dispatches the `Object` via `SubscriptionNotifier`. |
| `DisconnectWatchTask` | awaits transport close, then emits `SessionEvent::Disconnected`. |
| `PublishDoneExpiryTask` | receives the Request ID of every PUBLISH_DONE from `ControlMessageReceiveTask` and cancels that subscription 10 s later (see *Key invariants*). Holds only a `Weak<SessionContext>`. |

`Session::publisher()` / `subscriber()` return lightweight `Publisher` /
`Subscriber` facades sharing the same `Arc<SessionContext>`. Operations on an
established request take its handle, not its request id:
`Subscriber::unsubscribe` / `subscribe_update` / `update_forward` /
`fetch_relative_joining` take the `Subscription`, `Subscriber::fetch_cancel`
the `FetchHandle`, and `Publisher::publish_done` the `Subscription` it
publishes on. The request id stays readable as an identifier, but how a
request is cancelled or updated on the wire is left to moqt. Application
code consumes inbound control messages through
`Session::receive_event() -> SessionEvent`. `Session::closed()` awaits the
transport close itself (the signal `DisconnectWatchTask` turns into
`SessionEvent::Disconnected`) and returns the peer's termination code and
reason as `TransportClose`; it reads no session events, so a task can await it
beside `receive_event()`.
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
publisher: a `Clone` handle over `Arc<Session>` and a `tokio::sync::Mutex`
of per-track state, shared between the application's session event loop and
its media writers. Relays do not use it; they forward from their own cache
through the raw stream senders.

- Per track (`ensure_track(namespace, name, first_group_id, retention)`):
  `ObjectNumbering` (`GroupBoundary::{Within, Next, At}` → `Placement`),
  an `ObjectCache` with `Retention::For(duration)` or `Retention::NewestOnly`,
  the publication state, the optional `TrackWriter` and the largest location
  actually sent. The caller chooses the retention and the first group id.
- `write(namespace, name, OutgoingObject) -> WriteOutcome` numbers and
  caches the object first, whether or not the track is published, so the
  cache and the live stream always agree on ids. The writer is taken out of
  its slot for the duration of the awaited send and put back only if nothing
  replaced or released it meanwhile. `Sent(placement)` and
  `Stopped(placement)` (STOP_SENDING by the peer; the track resumes at the
  next group) carry the placement so the caller can account the stream;
  `NotSent` covers a `Within` object with no open group, a track without a
  writer, and a writer that still waits for the next group start.
  `write_group` sends one object as a finished group (the MSF catalog layout).
- `publish` sends PUBLISH and writes from the next group; it is a no-op while
  a subscription is served and after a PUBLISH was released, but publishes a
  track again whose SUBSCRIBE the peer released; `accept_subscribe`
  answers a SUBSCRIBE the application decided to accept (Content Exists is the
  largest sent location) and writes from the next group; `release(request_id)`
  drops the writer on UNSUBSCRIBE and returns the track so the caller can
  account it; `serve_fetch` answers a Standalone FETCH from the cache on its
  own stream (`FetchServeTask`, one per request, aborted by `cancel_fetch` and
  when the `Origin` is dropped; joining FETCH is NOT_SUPPORTED);
  `answer_track_status` fills TRACK_STATUS_OK like a SUBSCRIBE_OK.
- Left to the application: whether to accept a SUBSCRIBE, namespace
  announcements, when to PUBLISH, retention and first group ids, and
  disconnect handling. The FETCH End Location arithmetic is the free function
  `resolve_fetch_range` (`domains/fetch_range.rs`), shared with the wasm
  bindings.

### `SessionContext` — shared session state

One struct owns all cross-task state:

- `control: ControlChannel` — the version adapter above; for draft-14 it
  holds the request id counter and `inflight_requests`.
- `track_alias: AtomicU64` — per-session track alias allocation, shared by
  `SubscribeHandler::allocate_track_alias` and `Publisher::publish`.
- `object_sinks: HashMap<track_alias, ObjectSink>` — see buffering invariant
  below.
- `fetch_notification_map` / `fetch_receiver_map` — keyed by request id, used
  for FETCH data streams. A FETCH is answered on a single stream (draft-14
  §9.16.3), so `FetchNotifier` removes the sender when that stream arrives;
  the receiver keeps it until `accept_fetch_receiver`. Registration returns
  a `PendingFetch` drop-guard that removes both entries unless FETCH_OK
  arrived.

## Control plane (`modules/moqt/control_plane`)

- `control_messages/messages/*` — one struct per draft-14 control message with
  `encode`/`decode`. Framing (varint type + **16-bit fixed length**, a draft-14
  change) lives in `wire.rs`.
- `handler/*` — received-message facades handed to the application inside
  `SessionEvent` (e.g. `SubscribeHandler::ok(...)`, `::error(...)`). They keep
  an `Arc<SessionContext>` so responding does not require the `Session`.
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

Messages the crate can decode but not yet act on (GOAWAY, MAX_REQUEST_ID,
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
    do not increase. `TrackWriter` sends through the same two calls. Raw
    header and object-field sends exist only in tests of how a receiver
    resolves each header type and delta.
  - `StreamDataReceiver::receive()` returns `Subgroup::Header(SubgroupStart)`
    and then `Subgroup::Object(Object)` with absolute ids: header types
    without a Subgroup ID field mean subgroup 0, and the "first object id"
    types (0x12/0x13/0x1A/0x1B) are returned only once the first object
    arrived (a stream that ends before it yields no header). An Object Status
    draft-14 does not define is `StreamReceiveError::ProtocolViolation`.
  - `DatagramSender::send_object(&Object)` / `DatagramReceiver::receive()
    -> Object` do the same for datagrams (`ObjectDatagram::from_object` /
    `into_object`): an object id is always sent explicitly, a zero-length
    object is sent as its status (§10.2.1.1), and an object with a payload is
    always `Normal`: an End of Group type sets `Object::ends_group` instead of
    the status, and only a datagram with a payload sends it back. FETCH cannot
    carry it, so a fetched copy of that object has `ends_group` false.
  - `FetchDataSender::send_object(&Object)` / `FetchDataReceiver::receive()
    -> Fetch::{Object(Object), End}` do the same for FETCH responses
    (`FetchObjectField::from_object` / `into_object`): a datagram object is
    sent with its Object ID as the Subgroup ID (§10.4.4) and a zero-length
    object as its status. The FETCH_HEADER is consumed by moqt and not
    returned.
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
  `SessionContext::object_sinks`.
- `FetchNotifier` routes FETCH streams by request id as a
  `FetchDataReceiver`.
- `IncomingObject` is the per-track-alias envelope (`Subgroup` holding a
  `StreamDataReceiver` / `Datagram` holding an `Object`); FETCH streams never
  enter the per-track channel.

## Key invariants

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

## Testing conventions
Unit tests live in `#[cfg(test)] mod tests` inside the module under test
(Arrange/Act/Assert, see `wire.rs` framing round-trip tests). Cross-crate
behaviour is covered by the workspace-level E2E suites under `tests/`.

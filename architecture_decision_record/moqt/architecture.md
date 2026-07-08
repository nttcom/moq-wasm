# Architecture: `moqt` crate

## Status
Accepted (documents the design as of `master`, 2026-07)

## Purpose
`moqt` is the core implementation of Media over QUIC Transport
(draft-ietf-moq-transport-14). It provides both the client and server
endpoint roles and is the crate every other workspace member depends on
(`relay`, `bindings/wasm`, `bridges/*`, `examples/`).

The crate has two consumption modes:

1. **Full async endpoint** (native targets) — `Endpoint` / `Session` /
   `Publisher` / `Subscriber` running on tokio.
2. **Wire-format library** (`moqt::wire`, also compiled for `wasm32`) —
   message encode/decode only, no runtime. This is what `bindings/wasm`
   uses; everything runtime-related is gated behind
   `#[cfg(not(target_arch = "wasm32"))]`.

## Layer Map

```
moqt/src/modules/
├── transport/          Transport abstraction (QUIC / WebTransport / Dual)
│   ├── quic/           quinn-based raw QUIC (ALPN "moq-00")
│   ├── webtransport/   web_transport_quinn-based (browsers, ALPN h3)
│   └── dual/           One server endpoint accepting BOTH protocols
├── moqt/
│   ├── control_plane/  Control messages (encode/decode) + incoming-request handlers
│   ├── data_plane/     Object wire formats, stream/datagram senders & receivers, codecs
│   ├── domains/        Public API surface: Endpoint, Connecting, Session,
│   │                   Publisher, Subscriber, SessionContext
│   └── runtime/        Background receive tasks + object dispatch
└── extensions/         Buf{Get,Put}Ext (varint etc.), ResultExt
```

Dependency direction is strictly downward: `domains`/`runtime` →
`control_plane`/`data_plane` → `transport`. `transport` knows nothing
about MoQT semantics.

## Key Design Decisions

### 1. Transport selection via generics, not trait objects
`TransportProtocol` (`modules/moqt/protocol.rs`) is a marker trait with
associated types (`ConnectionCreator`, `Connection`, `SendStream`,
`ReceiveStream`). Concrete protocols are zero-sized types: `QUIC`,
`WEBTRANSPORT`, `DUAL`. All public types are generic over it
(`Endpoint<T>`, `Session<T>`, …).

- Rationale: no dynamic dispatch on the per-object hot path; the
  compiler monomorphizes each transport.
- Consequence: a `Session<QUIC>` and a `Session<WEBTRANSPORT>` are
  different types. Code that must hold sessions of mixed transports
  (the relay) erases the generic behind its own `dyn` traits — that is
  deliberately the *caller's* job, not this crate's.

### 2. `DUAL`: one UDP port, two protocols
`DUAL` is a server-only protocol whose creator registers two ALPNs
(`h3` for WebTransport, `moq-00` for raw QUIC) on a single quinn
endpoint and wraps the accepted connection in `DualConnection`. This
lets a server (typically the relay) serve browsers and native clients
on the same port. Client mode intentionally fails: a client always
knows which protocol it speaks.

### 3. Session establishment: `Endpoint` → `Connecting` → `Session`
`Endpoint::connect()/accept()` returns `Connecting<T>`, a boxed future
that performs the MoQT setup handshake (CLIENT_SETUP / SERVER_SETUP on
the bidirectional control stream) and resolves to a ready `Session<T>`.
This mirrors quinn's connect-then-await shape and keeps handshake logic
out of `Session`.

### 4. `Session` = thin façade over shared `SessionContext`
`Session<T>` owns the background task handles and an event receiver;
all shared mutable state lives in `Arc<SessionContext<T>>`
(`domains/session_context.rs`). `Publisher`/`Subscriber` are cheap
handles cloned from the same `Arc` (`session.publisher()`,
`session.subscriber()`), so one session can be used from both roles
concurrently. Dropping `Session` aborts all of its tasks.

`SessionContext` centralizes:
- **Request/response correlation** — `sender_map: RequestId →
  oneshot::Sender<ResponseMessage>`. Requesters register a sender, send
  the control message, and `await_response()` with a 10 s timeout
  (draft-14 §12.2); timeout closes the session with
  `ControlMessageTimeout`. `RegisteredSender` is a drop guard that
  removes the map entry even when the request future is cancelled.
- **ID allocation** — atomics for Request ID (step 2, per draft parity
  rule) and Track Alias.
- **Object routing** — see decision 6.

### 5. Background work as owned tasks (actor pattern)
Runtime behavior lives in four task structs under `runtime/tasks/`
(`ControlMessageReceiveTask`, `UniStreamReceiveTask`,
`DatagramReceiveTask`, `DisconnectWatchTask`), each following the
repo-wide convention: `run()` spawns and returns the struct owning the
`JoinHandle`. The control-message task holds only a
`Weak<SessionContext>` so a dropped `Session` can actually deallocate.

Incoming control messages are split into two kinds:
- **Responses** (SUBSCRIBE_OK, REQUEST_ERROR, …) are routed to the
  waiting requester through `sender_map`. A response for an unknown
  Request ID is a protocol violation and closes the session.
- **Requests** from the peer (SUBSCRIBE, PUBLISH_NAMESPACE, FETCH, …)
  become `SessionEvent`s delivered to the application via
  `Session::receive_event()`.

### 6. Incoming requests are surfaced as handler objects
Each `SessionEvent` variant carries a handler
(`control_plane/handler/*_handler.rs`, e.g. `SubscribeHandler`) that
exposes the request's fields plus explicit `ok(...)` / `error(...)`
methods that send the response. The application decides; the library
never auto-accepts. This is the seam the relay builds on (it wraps
these handlers in its own `dyn` traits).

### 7. Object delivery: buffer-until-receiver, then live channel
Objects can arrive on unidirectional streams *before* the local
SUBSCRIBE_OK plumbing registered a receiver for the track alias.
`SessionContext::object_sinks` holds, per track alias, either a
`Buffer(VecDeque)` (pre-registration, bounded by `max_pending_objects`,
drop-oldest) or a `Receiver(mpsc sender)` (live). Registration
atomically drains the buffer into the new channel under the same lock.
Duplicate registration for a track alias is rejected as
`DuplicateTrackAlias`.

### 8. Control plane / data plane split mirrors the draft
- `control_plane/control_messages/` — one file per draft message with
  `encode()`/`decode()` and unit tests; shared parameter types under
  `messages/parameters/`. Message framing (type varint + 16-bit length,
  draft-14) lives in `wire.rs`.
- `data_plane/object/` — data wire formats (subgroup headers/objects,
  datagrams, fetch objects, extension headers).
- `data_plane/stream/`, `data_plane/datagram/` — typed senders and
  receivers over transport streams; `data_plane/codec/` — incremental
  decoders driving them.

### 9. Typestate for stream senders
`StreamDataSender<T, S>` uses a typestate parameter
(`Uninitialized` → `HeaderSent`): `send_header()` consumes the sender
and returns the object-sending state, so "subgroup header is sent
exactly once, before any object" is a compile-time invariant instead of
a runtime check.

### 10. Public API is curated in `lib.rs`
Everything under `modules/` is `pub(crate)`; `lib.rs` re-exports the
intended surface explicitly (and `wire.rs` re-exports the encode/decode
surface). Adding a `pub` item to a module does not leak it — it must
also be re-exported, which keeps the API reviewable.

## Consequences
- New control messages follow a fixed recipe: message struct +
  `ControlMessageType` entry (+ handler and `SessionEvent` variant if it
  is a request), and dispatch in the control-message receive task.
- Transport-generic code stays monomorphic and fast, but any component
  that mixes transports at runtime must do its own type erasure (see
  `relay`'s `core` module, documented in
  `../relay/architecture.md`).
- The wasm boundary must be maintained by hand: runtime/domain code
  needs the `#[cfg(not(target_arch = "wasm32"))]` gate, wire-format code
  must stay runtime-free.

## References
- `spec/draft-ietf-moq-transport-14.txt` (authoritative draft for this crate)
- `../relay/architecture.md` — how the relay consumes this crate

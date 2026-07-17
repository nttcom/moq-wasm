# `moqt` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
The `moqt` crate is the core implementation of Media over QUIC Transport
(draft-ietf-moq-transport-14). Every other crate in the workspace depends on it:
`relay` builds a server on top of it, `bindings/wasm` reuses its message codecs,
and the bridges/examples use its client API.

## Layering

```
lib.rs (public re-exports)        wire.rs (raw message codecs, wasm-safe)
        │
        ▼
modules/moqt/domains      ← public API objects (Endpoint, Session, Publisher, …)
modules/moqt/runtime      ← background tasks + incoming-object dispatch
modules/moqt/control_plane← control messages, handlers, session events
modules/moqt/data_plane   ← object formats, stream/datagram send/receive, codecs
modules/moqt/protocol.rs  ← TransportProtocol trait + QUIC/WEBTRANSPORT/DUAL markers
        │
        ▼
modules/transport         ← transport abstraction + quinn/web-transport-quinn impls
```

- `lib.rs` re-exports the session-level API. Everything except the message
  codecs is `#[cfg(not(target_arch = "wasm32"))]`; on wasm32 only
  `control_plane` and `data_plane` compile, which is what `bindings/wasm`
  consumes.
- `wire.rs` re-exports raw control-message structs, framing helpers
  (`encode_control_message` / `take_control_message`), and data-plane object
  types for consumers that need direct wire access (the relay uses
  `moqt::wire::FetchType`, the wasm bindings use the codecs).

## Transport layer (`modules/transport`)

`protocol.rs` defines the `TransportProtocol` trait with four associated types:

```rust
trait TransportProtocol {
    type ConnectionCreator: TransportConnectionCreator;
    type Connection: TransportConnection;
    type SendStream: TransportSendStream;
    type ReceiveStream: TransportReceiveStream;
}
```

Three zero-sized markers implement it:

| Marker | Implementation | Notes |
| --- | --- | --- |
| `QUIC` | quinn, ALPN `moq-00` | raw QUIC; used for inter-relay links and native clients |
| `WEBTRANSPORT` | web-transport-quinn, ALPN `h3` | browser-facing |
| `DUAL` | quinn endpoint dispatching on negotiated ALPN | **server-only**; accepts both `h3` (WebTransport handshake) and `moq-00` (raw QUIC) on one port. Client-side constructors `bail!`. |

The whole session stack is generic over `T: TransportProtocol`, so protocol
selection is a compile-time type parameter (e.g. `Endpoint::<moqt::DUAL>`),
not a runtime branch — except inside `DualConnection`, which wraps either
variant behind one connection type.

## Session establishment (`modules/moqt/domains`)

Flow: `Endpoint` → `Connecting` (a boxed `Future`) → `Session`.

- `Endpoint::create_client(&ClientConfig)` / `create_server(&ServerConfig)`
  build a `SessionCreator` around the transport's `ConnectionCreator`.
- `connect()` / `accept()` return `Connecting<T>`, whose future performs the
  transport handshake, opens/accepts the bidirectional control stream, and runs
  the SETUP exchange in `SessionContextFactory` (CLIENT_SETUP/SERVER_SETUP,
  version `0xff00000e` = draft-14).
- On success a `Session<T>` is created.

### `Session` and its background tasks

`Session::new` spawns four tasks (all named via `tokio::task::Builder`, all
aborted in `Drop`):

| Task | Role |
| --- | --- |
| `ControlMessageReceiveTask` | reads the control stream, decodes messages, routes them (see below). Holds only a `Weak<SessionContext>` so it cannot keep the session alive. |
| `UniStreamReceiveTask` | accepts incoming unidirectional streams; the first frame must be a subgroup header (→ `SubscriptionNotifier`) or fetch header (→ `FetchNotifier`). |
| `DatagramReceiveTask` | receives datagrams, decodes `ObjectDatagram`, dispatches via `SubscriptionNotifier`. |
| `DisconnectWatchTask` | awaits transport close, then emits `SessionEvent::Disconnected`. |

`Session::publisher()` / `subscriber()` return lightweight `Publisher<T>` /
`Subscriber<T>` facades sharing the same `Arc<SessionContext<T>>`. Application
code consumes inbound control messages through
`Session::receive_event() -> SessionEvent<T>`.

### `SessionContext` — shared session state

One struct owns all cross-task state:

- `request_id: AtomicU64` — starts at 1, incremented by 2 per request.
- `track_alias: AtomicU64` — server-side track alias allocation
  (`SubscribeHandler::allocate_track_alias`). `Publisher::publish` uses a
  separate process-global `NEXT_TRACK_ALIAS` counter.
- `sender_map: HashMap<RequestId, oneshot::Sender<ResponseMessage>>` —
  request/response correlation. Registration returns a `RegisteredSender`
  drop-guard that removes the entry on drop, so cancelled requests never leak
  senders.
- `object_sinks: HashMap<track_alias, ObjectSink>` — see buffering invariant
  below.
- `fetch_notification_map` / `fetch_receiver_map` — keyed by request id, used
  for FETCH data streams.

## Control plane (`modules/moqt/control_plane`)

- `control_messages/messages/*` — one struct per draft-14 control message with
  `encode`/`decode`. Framing (varint type + **16-bit fixed length**, a draft-14
  change) lives in `wire.rs`.
- `handler/*` — received-message facades handed to the application inside
  `SessionEvent` (e.g. `SubscribeHandler::ok(...)`, `::error(...)`). They keep
  an `Arc<SessionContext>` so responding does not require the `Session`.
- `enums.rs` — `SessionEvent<T>` (inbound requests + `Disconnected` /
  `ProtocolViolation`) and the crate-private `ResponseMessage`.
- `constants.rs` — protocol version and `TerminationErrorCode` (draft-14
  §13.1.1).

`ControlMessageReceiveTask` splits every decoded message into one of two paths:

1. **Requests** (SUBSCRIBE, PUBLISH, FETCH, namespace messages, …) become
   `SessionEvent` variants delivered to `Session::receive_event()`.
2. **Responses** (`*_OK` / `*_ERROR`) are matched against `sender_map` by
   request id and complete the pending `oneshot`.

## Data plane (`modules/moqt/data_plane`)

- `object/*` — wire formats: `SubgroupHeader` (types `0x10..=0x1D` encoding
  subgroup-id mode / extensions / end-of-group), `SubgroupObject`,
  `ObjectDatagram`, `FetchObject`, `ExtensionHeaders`, `ObjectStatus`.
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
    returns a `StreamDataSender`.
  - `StreamDataSender` uses a **typestate** (`Uninitialized` → `send_header()`
    → `HeaderSent`) so "header before objects" is enforced at compile time.
  - `StreamDataReceiverFactory` / `StreamDataReceiver`, `DatagramSender` /
    `DatagramReceiver`, `FetchDataSender` / `FetchDataReceiver` mirror this on
    the other side.

## Runtime dispatch (`modules/moqt/runtime/dispatch`)

- `SubscriptionNotifier` routes incoming objects by track alias into
  `SessionContext::object_sinks`.
- `FetchNotifier` routes FETCH streams by request id.
- `IncomingObject<T>` is the internal envelope (`StreamHeader` / `Datagram` /
  `Fetch`).

## Key invariants

- **Buffer-before-subscribe**: objects can arrive before the application
  registers a receiver (data races SUBSCRIBE_OK). `object_sinks` buffers up to
  256 objects per track alias (`SubscriptionNotifier::MAX_PENDING_OBJECTS_PER_TRACK_ALIAS`),
  dropping the oldest on overflow. `register_data_receiver` drains the buffer
  into the new channel atomically under the sinks lock.
- **Duplicate track alias**: if SUBSCRIBE_OK carries a track alias that already
  has a registered receiver, the subscriber closes the session with
  `DuplicateTrackAlias` (draft-14 §9.8).
- **Unknown response request id**: a response that matches no `sender_map`
  entry is a protocol violation; the session is closed with
  `ProtocolViolation`.
- **Control response timeout**: `SessionContext::await_response` bounds every
  request/response wait to 10 s; on timeout the session is closed with
  `ControlMessageTimeout` (draft-14 §12.2).
- **Header-first subgroup streams**: enforced by the sender typestate; on the
  receive side a uni stream whose first frame is not a header is rejected.
- **Session teardown**: dropping `Session` aborts all four background tasks;
  the control task's `Weak` reference guarantees it never keeps the context
  alive.

## Testing conventions
Unit tests live in `#[cfg(test)] mod tests` inside the module under test
(Arrange/Act/Assert, see `wire.rs` framing round-trip tests). Cross-crate
behaviour is covered by the workspace-level E2E suites under `tests/`.

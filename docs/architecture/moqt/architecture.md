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
modules/moqt/protocol.rs  ← TransportProtocol trait + QUIC/NATIVE markers
        │
        ▼
modules/transport         ← transport abstraction + quinn/web-transport-quinn impls + QMux over WebSocket
modules/executor          ← task spawning, yielding, timeouts and the monotonic clock (tokio natively, wasm-bindgen-futures / performance.now on wasm32)
```

- The public API is grouped by MoQT operation, so the type that sends a
  request and the handler that answers it share a module:

  | Path | Contents |
  | --- | --- |
  | `moqt::` | `Endpoint`, `Session`, `SessionEvent`, `Publisher`, `Subscriber`, `Subscription`, `TrackWriter` / `TrackReader`, the transport markers, session-wide handlers (GOAWAY) and errors not tied to one operation |
  | `moqt::subscribe` | SUBSCRIBE, SUBSCRIBE_UPDATE, UNSUBSCRIBE and TRACK_STATUS: options, `FilterType`, handlers |
  | `moqt::publish` | PUBLISH and PUBLISH_DONE: `PublishOption`, handlers |
  | `moqt::fetch` | FETCH and FETCH_CANCEL: options, `FetchParams`, handlers, fetch stream sender/receiver |
  | `moqt::namespace` | PUBLISH_NAMESPACE, PUBLISH_NAMESPACE_DONE, PUBLISH_NAMESPACE_CANCEL, SUBSCRIBE_NAMESPACE (and draft-18 SUBSCRIBE_TRACKS) and UNSUBSCRIBE_NAMESPACE: `PendingPublishNamespace`, `NamespaceInterest`, handlers |
  | `moqt::object` | Draft-independent object model (`Object`, `ForwardingPreference`, `SubgroupStart`, `Location`, `GroupOrder`, `ExtensionHeaders`, `ObjectStatus`) and the subgroup stream / datagram senders and receivers both SUBSCRIBE and PUBLISH deliver over |
  | `moqt::server` | `Accepting`, `Handshake`, `ServerConfig`, `TransportAddresses` |
  | `moqt::executor` | `spawn` / `JoinHandle`, `sleep`, the monotonic clock `now` and the wall clock `system_time`, for crates built on moqt that also run on wasm32 |

  Implementation files under `modules/` are not arranged by these modules;
  each public module only re-exports.
- The whole stack compiles for `wasm32` as well; the quinn-based transports
  and their markers (`QUIC`, `NATIVE`) are `#[cfg(not(target_arch = "wasm32"))]`
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
connection exists. `TransportReceiveStream::stop` sends STOP_SENDING with a
code (quinn `RecvStream::stop`, web-transport-quinn `RecvStream::stop`,
`ReadableStreamDefaultReader.cancel` with a `WebTransportError` in the
browser); `TransportSendStream::poll_stopped` yields the peer's STOP_SENDING
code (quinn `SendStream::stopped`, web-transport-quinn `poll_closed`, the
rejection of `writer.closed` in the browser), keeping its wait inside the
stream between polls so that `StreamSender::stopped` can wait without
holding the stream. `try_open_bi` opens a bidirectional stream only when the
peer's stream limit allows one now: QUIC polls quinn's open once, which
takes no stream ID while pending; WebTransport keeps a pending open for the
next call, since past the stream ID it still writes the stream header; the
browser's API cannot tell, so it waits like `open_bi`.

Three zero-sized markers implement it; the first two are native-only:

| Marker | Implementation | Notes |
| --- | --- | --- |
| `QUIC` | quinn, the MoQT ALPNs (`moq-00`) | raw QUIC; used for inter-relay links and native clients |
| `NATIVE` | quinn endpoint dispatching on ALPN, plus a TCP listener | server: accepts both `h3` (WebTransport handshake) and the MoQT ALPNs (raw QUIC) on one UDP port, and QMux over WebSocket (`wss://`, below) on the TCP port of the same number. Client: one UDP socket, the URL scheme picks the ALPN per connection; both connect through `ConnectTarget::connect_quic` (IPv4 preferred, matching the IPv4 client socket), and WebTransport then runs `web_transport_quinn::Session::connect` on that connection. A `wss://` URL opens a TCP + TLS connection instead. |
| `BROWSER` | the browser's `WebTransport` API through `web-sys` (`modules/transport/browser`), and QMux over its `WebSocket` (`browser_web_socket.rs`) | wasm32 only, client only: `https://` and `wss://` URLs, only `CertificateVerification::NativeRoots` (the browser's own trust store), `accept` and `server` fail. Stream priority maps to `sendOrder`, RESET_STREAM and STOP_SENDING codes to `WebTransportError.streamErrorCode`, datagrams are written fire-and-forget. `TransportSendStream::close` is synchronous on every transport, so it cannot wait for the peer's ACK of the FIN; here it starts `writer.close()`, which resolves only once that ACK arrives, and returns without awaiting it. `stats()` is all zeros. The `web-sys` WebTransport types are behind `--cfg web_sys_unstable_apis`, set in the root `.cargo/config.toml`. A WebTransport that loses the race, to WebSocket or to the hedged second WebTransport, is closed, since the JavaScript object would keep connecting. Over WebSocket, `send` waits while the browser has more than 1 MiB queued (`bufferedAmount`), so a slow TCP path pushes back on the QMux send task; a browser cannot send WebSocket Ping, and QX_PING keeps the session alive. |

Only `Endpoint<T>` and its `SessionCreator<T>` are generic over
`T: TransportProtocol`, because the creator's constructors are static
(`T::ConnectionCreator::client` / `server`). Everything an endpoint produces
(`Connecting`, `Accepting`, `Handshake`, `Session`, the handlers and the data
stream types) is a plain type over the boxed transport, so one process can hold
sessions of different transports in one collection and the `NATIVE` creator
simply boxes a `QUICConnection` or a `WtConnection` per connection.

Client-side helpers shared by the creators:

- `connect_target.rs` — `ConnectTarget::parse(url)` maps the scheme to a
  transport (`moqt://` → raw QUIC, default port 4433; `https://` → WebTransport,
  default port 443; `wss://` → QMux over WebSocket, default port 443) and
  resolves the host, preferring IPv4 because the client endpoint binds an IPv4
  socket. The `QUIC` creator rejects a URL for another transport.
  The wasm build parses with the browser's `URL`
  (`web_sys::Url`) instead of the `url` crate and leaves host and port to
  `WebTransport`.
- WebSocket fallback — `SessionCreator::open_transport` (shared by every
  marker and target) races the transport of a `moqt://` or `https://` URL
  against `wss://` on the same host and port (`ConnectTarget::web_socket_fallback`;
  `moqt://` has no path, so `/`), as Happy Eyeballs (RFC 8305) does: the
  first gets a head start, then `wss://` is dialed alongside it, the first
  to open is used and the other is dropped. A failure within the head start
  dials `wss://` at once, so a browser without the WebTransport API does not
  wait. Only creators with a `WEB_SOCKET_HEAD_START` do this, `NATIVE` (200 ms)
  and `BROWSER` (500 ms), so `QUIC` links between relays never move to TCP.
  Which transport won is not remembered: each connect races again,
  and a session that landed on WebSocket stays there until it ends. The
  host is resolved once before the head start, so a slow DNS server does
  not use it up, and both dials go to the same address. Only opening the
  transport falls back; a failed SETUP is returned as is. A WebSocket that
  wins is logged at WARN with `url` and `fallback_url`. When both fail, the
  error names both. `BROWSER` also hedges WebTransport: a second one dials
  at 500 ms, since a first whose initial packet was lost waits about a
  second for QUIC to resend it and would lose to WebSocket.
- `client_crypto.rs` — the rustls client config for
  `ClientConfig::certificate_verification` (native roots, the certificates of
  one PEM file, or no verification), the `quinn::ClientConfig` per ALPN, and
  the client `quinn::Endpoint`. The config is built once per `Endpoint`, so
  every connection it opens, including those a session opens to follow
  GOAWAY or to reconnect, verifies the server the same way.

The two native creators bind server endpoints to `[::]:port` through
`bind_dual_stack` (`dual_stack_bind.rs`). On Apple targets it holds
`0.0.0.0:port` during the bind, because XNU lets `[::]:port` bind while
another socket holds `0.0.0.0:port`. XNU picks that IPv4 port without regard
to sockets bound to a specific IPv6 address, such as a browser's QUIC
connections, so for port 0 a `[::]:port` already in use draws another number,
up to `EPHEMERAL_PORT_ATTEMPTS` times.

The creators do not know MoQT versions. Each takes an
`ApplicationProtocols` (`application_protocol.rs`): the ALPNs a raw QUIC
connection offers or accepts, and the WebTransport subprotocols a client
lists in WT-Available-Protocols. A WebTransport server answers with the first
of its own subprotocols the client also offered (WT-Protocol), or with none.
A WebSocket client offers the QMux subprotocols derived from the ALPNs
(`qmux-02.<alpn>`, then the bare `qmux-02`; `qmux/subprotocol.rs`) in
Sec-WebSocket-Protocol, and the server answers with the first of its own
the client offered, or fails the upgrade with 400.
Every connection reports what its handshake settled on through
`TransportConnection::negotiated_protocol()`; `Version::negotiated` maps the
`moq-00` ALPN, a WebTransport session without a subprotocol and a WebSocket
subprotocol naming `moq-00` or nothing to draft-14.
`BROWSER` sets `WebTransportOptions.protocols` and reads `WebTransport.protocol`
through `js_sys::Reflect`, since web-sys exposes neither; a browser without
subprotocol support reports none.

### QMux over WebSocket (`modules/transport/qmux`, `modules/transport/websocket`)

The fallback for clients that cannot use QUIC: UDP-blocked networks and
browsers without WebTransport. QMux (draft-ietf-quic-qmux-02) carries the
QUIC version 1 frames — STREAM, RESET_STREAM, STOP_SENDING, MAX_DATA,
MAX_STREAM_DATA, MAX_STREAMS, the BLOCKED frames, CONNECTION_CLOSE and
DATAGRAM — over an ordered, reliable transport, so the stream model above the
`TransportConnection` trait is unchanged. The WebSocket binding
(draft-lcurley-qmux-websocket-00) makes each binary message one record
without the `Size` prefix and negotiates the application protocol through
the subprotocol. `modules/transport/qmux` is written for native and wasm32
alike; only the message transport under it differs.

- `message_transport.rs` — `MessageSink` / `MessageSource`, the two halves of
  the transport a `QmuxConnection` runs on. `websocket/tungstenite_transport.rs`
  implements them on a `tokio_tungstenite::WebSocketStream`; `qmux/in_memory.rs`
  on channels, for tests.
- `frame.rs`, `transport_parameters.rs` — the codecs. The first frame each
  side sends is QX_TRANSPORT_PARAMETERS; the receive task rejects any other
  first frame, a second one, a prohibited frame, a truncated record, a
  record above `max_record_size` (16382, never raised) and a forbidden
  transport parameter with the QUIC error code the draft names.
- `qmux_connection.rs` — `QmuxConnection::establish` sends the local
  parameters, spawns the two tasks and resolves once the peer's parameters
  arrive. Local limits follow quinn's: 1 MiB per stream
  (`stream_receive_window`), 1000 unidirectional streams
  (`max_concurrent_uni_streams`), 100 bidirectional streams, 16 MiB for the
  connection, 30 s idle timeout, datagrams up to one record; outgoing
  datagrams wait in a 1 MiB buffer (quinn's `datagram_send_buffer_size`)
  that drops the oldest when full.
- `tasks/qmux_send_task.rs` — the actor that owns the sink. It builds each
  record from control frames first, then datagrams, then stream data in
  `set_priority` order (higher first, equal priorities round-robin), within
  the peer's credit and record size. Backpressure reaches
  `TransportSendStream::send` through a 64 KiB queue per stream
  (`SendQuota`), not through the command channel, so a low-priority stream
  that fills its queue does not hold a high-priority one. What the peer
  grants or asks (MAX_DATA, MAX_STREAM_DATA, STOP_SENDING, QX_PING, a freed
  stream slot) is written to shared state and the task woken at most once
  per pass (`SessionState::wake`), so a peer that stops reading its socket
  cannot queue work in the relay. A QX_PING every 5 s is the keep-alive (a
  browser cannot send WebSocket Ping) and the source of
  `TransportStats::rtt`.
- `tasks/qmux_receive_task.rs` — reads records and hands frames to the
  stream they belong to. **It never waits for the application** (draft §6):
  data goes into the stream's buffer, bounded by the window this side
  advertised; MAX_STREAM_DATA and MAX_DATA are raised once half a window has
  been read (`qmux_receive_stream.rs`, `SessionState::consumed`). A
  peer-opened stream's MAX_STREAMS slot is returned once the application has
  dropped its handle and the peer's data has ended, as quinn does, so neither
  unread streams nor a peer that ignores STOP_SENDING can grow the session
  past the limit. Nothing
  received for 30 s ends the session, and so does a record the peer has not
  taken for 30 s.
- Close: `TransportConnection::close` sends CONNECTION_CLOSE 0x1d with the
  MoQT termination code, then the WebSocket Close frame; a peer's 0x1d is
  reported as `TransportClose { code: Some(..) }`, a transport error (0x1c)
  or a lost socket as `code: None`, like quinn's `ApplicationClosed`. Dropping
  a `QmuxSendStream` before `close` resets it with code 0 and dropping a
  `QmuxReceiveStream` before its end sends STOP_SENDING 0, as quinn does.
- `websocket/websocket_listener.rs` — binds `[::]:port` (TCP), terminates
  TLS with the server's certificate (ALPN `http/1.1`), runs the HTTP/1.1
  upgrade with the subprotocol selection in a task per connection (10 s
  limit) and answers 503 while the endpoint refuses new connections. The
  `NATIVE` creator counts every WebSocket session it hands out, accepted or
  connected, until both its send and receive tasks have returned
  (`TransportEnded`): the last CONNECTION_CLOSE and WebSocket Close are
  written and the receive task has drained until the peer's Close, so
  `wait_idle` does not close a socket with unread bytes, which would answer
  with RST. `websocket/websocket_client.rs` is the
  `wss://` client; it fails the handshake when the server selects no
  offered subprotocol.
- `Session::url_scheme` is `wss` for a WebSocket session, so a GOAWAY's New
  Session URI keeps a session that landed on WebSocket there.

## Version adapter (`modules/moqt/version`)

Everything above the adapter states what to send and receive; the adapter
decides how a draft carries it. draft-14 and draft-18 are implemented.
`ClientConfig::versions` and `ServerConfig::versions` (public `moqt::Version`,
most preferred first) choose the drafts an endpoint offers; the default,
`DEFAULT_VERSIONS`, prefers draft-18 and falls back to draft-14.

A draft is added under `modules/moqt/version/<draft>` and the wire layer.
`modules/moqt/runtime/tasks` and `modules/transport` change only to route a
call through the adapter; a draft never gets a task of its own (see
*Performance invariants*).

- `Version` names the draft a session speaks.
- `ControlChannel` is an enum with one variant per draft, matched in place
  instead of a trait object: the set of drafts is closed, the match is
  checked for exhaustiveness when a draft is added, and async methods need no
  object safety or `Send` bounds that wasm32 cannot meet. It offers
  `reserve_request_id`, `send_request(RequestIdReservation, OutgoingRequest)
  -> PendingResponse`, `send_subscribe_update`,
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
- `version::application_protocols(versions)` lists the ALPN and WebTransport
  subprotocol of every draft in the config, in its order; `Endpoint` hands
  them to its creator. A server takes the first of its own ALPNs or
  subprotocols the client also offered, so the server's order decides.
  `server_application_protocols` leaves out the subprotocols listed after
  draft-14: a WebTransport session without a subprotocol is draft-14, so a
  server listing draft-14 first speaks draft-14 over WebTransport.
  `version::connect` / `accept` pick the `Version` from the connection's
  `negotiated_protocol()` before running that draft's SETUP exchange: ALPN
  `moq-00` (draft-14 §3.1.2) and a WebTransport session without a subprotocol
  (draft-14 defines none, §3.1.1) are draft-14; ALPN or subprotocol `moqt-18`
  is draft-18; anything else, or a draft the endpoint's config does not
  list, fails the session, so a server that accepts only draft-18 refuses a
  WebTransport session without a subprotocol.
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
    Cancelling also notifies the stream's `reader_stop`, on which the task
    reading the peer's half stops it with STOP_SENDING CANCELLED; §5.2
    requires that of a FETCH requester. When the peer ends a request, this
    side's half ends with FIN.
  - A FETCH's state lasts until its response stream ends (§5.2). The
    publisher ends the request stream with FIN once both its FETCH_OK and the
    FIN or reset of the response stream (`FetchDataSender::close` /
    `reset`, or its drop, which resets an unfinished response) are sent. The requester ends its half with FIN when the
    `FetchDataReceiver` reads the response stream's end or reset
    (`FetchRequestEnd`), and cancels the FETCH when the receiver is dropped
    before. A FIN from the requester does not cancel a FETCH; a reset of
    its half does, and so does STOP_SENDING on the publisher's half, which
    the task reading the stream watches (`StreamSender::stopped`), or on
    the response stream, which `FetchDataSender` notices when it sends or
    closes. The
    publisher then resets both the request stream and the response stream
    and raises `FetchCancel`: `FetchDataSender` attaches its stream to the
    request (`attach_fetch_response`), and a response stream opened for a
    FETCH no longer open is reset at once.
  - `request_streams` reads the streams. A request the peer opens becomes the
    draft-14 event once its Request ID passes the parity and reuse checks
    (§10.1). The peer ending an open request raises the event its draft-14
    cancellation would (`Unsubscribe`, `FetchCancel`, `PublishNamespaceDone`,
    `UnsubscribeNamespace`, `PublishDone`). PUBLISH_DONE is the publisher's
    final message on a subscription's stream (§10.11), so this side stops
    reading the stream there and ends its half with FIN.
  - `messages` / `parameters` encode the draft-14 structs: Message Parameters
    and Track Properties carry the former fields, absent ones take their
    draft-18 defaults, and responses take their Request ID from the stream.
    A Track Property is looked up in the mutable list and inside Immutable
    Properties (§12.7).
    - `TrackProperties` holds every received Track Property with the bytes
      it arrived in, and `put_track_properties` sends those bytes again, so
      a relay forwards a property unchanged (§2.5), a non-minimal varint and
      Immutable Properties (§12.7) included, and adds no default it did not
      receive (an omitted DEFAULT PUBLISHER GROUP ORDER stays omitted). A
      property set through a setter is encoded afresh, and so is the Delta
      Type of the property after it when the type before it changed.
    - A PUBLISH, SUBSCRIBE_OK or FETCH_OK with malformed Immutable
      Properties (§12.7) is refused with MALFORMED_TRACK, and one with a
      Mandatory Track Property (§2.5.1) with UNSUPPORTED_EXTENSION, before
      it reaches the model, so the model never holds one from the peer.
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
      NAMESPACE on that stream, once per namespace, and accepted at once.
      While the SUBSCRIBE_NAMESPACE is unanswered the namespace is held and
      sent after its REQUEST_OK, which §10.18 makes the first message of the
      response; a REQUEST_ERROR or a `publish_namespace_done` drops it.
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
      end of the stream does (§10.18). On a request the peer opened it is
      ignored, but a second one on the same stream, or one with a New
      Session URI sent to a server, closes the session with
      PROTOCOL_VIOLATION (§10.4).
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
      A NEW_GROUP_REQUEST (§10.2.13) the update carries is read with
      `SubscribeUpdateHandler::new_group_request()`. It belongs to that
      update only and is not part of the state the next update starts from.
    - A REQUEST_UPDATE with TRACK_NAMESPACE_PREFIX on the peer's
      SUBSCRIBE_NAMESPACE or SUBSCRIBE_TRACKS raises
      `SubscribeNamespaceUpdate` (§10.9.2), which draft-14 never raises; the
      application decides the PREFIX_OVERLAP rule (§10.2.14). Accepting moves
      the stream's prefix and answers REQUEST_OK under the channel's
      `namespace_writes` lock, which every NAMESPACE / NAMESPACE_DONE also
      takes, so later suffixes are relative to the new prefix; namespaces
      the new prefix leaves out are forgotten. Refusing, dropping the
      handler (NOT_SUPPORTED) or leaving it unanswered for the control
      message timeout (TIMEOUT) answers REQUEST_ERROR, ends the stream with
      FIN (§10.9.1) and raises `UnsubscribeNamespace` for the old prefix;
      a later `ok` returns `PrefixUpdateAnswer::Failed`, as it does when the
      subscription ended before the answer. These answers share the reading
      model of a subscription's updates (`UpdateAnswerSlot` and the stream's
      ordered answer queue): the stream goes on being read, a later update
      moving on from the prefix an earlier pending one asks for, and the
      answers go out in arrival order. Without TRACK_NAMESPACE_PREFIX a
      namespace stream's REQUEST_UPDATE gets an empty REQUEST_OK in its
      turn.
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
  `USE_VALUE`, Token Type `0`, UTF-8 value). It is an
  `AuthorizationTokenSource`: a fixed string, or a provider that
  `SessionCreator::set_up` calls before each CLIENT_SETUP, on the first
  connection and on every connection a session opens to follow GOAWAY, so a
  client with short-lived tokens fetches a new one each time. A provider that
  fails fails that connection. On success a `Session` is created.
- `refuse_new_connections()` makes every later connection attempt fail at
  the transport level (QUIC CONNECTION_REFUSED, HTTP 503 for WebTransport)
  while `accept()` keeps being polled; established sessions are unaffected.
  A closing server uses it so that clients learn at once instead of waiting
  for their handshake to time out.
- `accept()` returns `Accepting`, or `AcceptError::EndpointClosing` once the
  endpoint is closed, which every transport reports the same way. `accept()`
  does not wait on the peer: `TransportConnectionCreator::accept_new_transport`
  returns the rest of the transport handshake (on `NATIVE`: QUIC, then the
  WebTransport CONNECT on `h3`) as a `TransportAccepting` future, so a server
  that spawns each `Accepting` keeps accepting while one peer stalls. A
  transport handshake that fails therefore fails `Accepting`, not `accept()`.
  The future of `Accepting` runs that handshake, failing it after 10 s
  (`TRANSPORT_HANDSHAKE_TIMEOUT`), then `version::accept`, which for draft-14
  accepts the control stream and stops after CLIENT_SETUP is received. When
  CLIENT_SETUP does not arrive within another 10 s (`CLIENT_SETUP_TIMEOUT`),
  the future closes the connection with CONTROL_MESSAGE_TIMEOUT. It resolves
  to a
  `Handshake` that exposes the CLIENT_SETUP's AUTHORIZATION TOKEN parameters
  (`authorization_tokens()`) so the application can decide before
  SERVER_SETUP is sent:
  - `Handshake::accept(session_span)` answers the setup (SERVER_SETUP for
    draft-14) in the current span and creates the `Session`, whose tasks
    run under `session_span`.
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
| `PublishDoneExpiryTask` | receives every PUBLISH_DONE from `ControlMessageReceiveTask` and cancels that subscription of its connection once the streams its Stream Count counts have arrived, or 10 s later (see *Key invariants*). Holds only `Weak` references. |

A client session with `ClientConfig::follow_go_away` (the default) follows
the server's GOAWAY by itself (draft-14 §3.5; see
[session-migration.md](session-migration.md)). Its connections send their
events to a `MigrationTask` (`domains/session/migration_task.rs`) instead of
the application: it forwards them with logical ids, and on GOAWAY it opens
the next connection through the endpoint's `SessionCreator` (`Reconnect`),
sends every namespace subscription (with its `NamespaceInterest`), PUBLISH_NAMESPACE, PUBLISH and SUBSCRIBE
made on the session again (`NextGroupStart`, or an AbsoluteRange from the
group after the newest one delivered to its End Group, which is not sent
again once that group has started; the range, Forward state and priority
of the last SUBSCRIBE_UPDATE; a
rejected namespace or SUBSCRIBE is repeated every 500 ms for up to 10 s; the
connection keeps the Start Location the new SUBSCRIBE_OK resolves, which
`update_forward` and `end_before_group` repeat there when it is later),
makes it the current connection (one of those four requests that a lost
connection cut off before its answer is sent again there), and closes the previous one with NO_ERROR
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
once it ran out. A Relative Joining FETCH made after the move goes as a
Standalone FETCH of the same range, since the current connection holds the
subscription with the filter it was sent again with. While both
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
request is cancelled or updated on the wire is left to moqt.
`update_forward` and `end_before_group` take the fields they do not set
from the last SUBSCRIBE_UPDATE sent for the `Subscription`, which its
clones share, so neither widens a range an earlier update narrowed
(§9.10); before any update they take them from its SUBSCRIBE or PUBLISH_OK,
the Forward State included. Application
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
  `first_group_id` given at construction unless chosen, object ids run from
  0 within each group;
  publisher priority is fixed at 128 and the subgroup id is omitted
  (`SubgroupId::None`).
- `TrackReader` wraps a `DataReceiver` and yields
  `TrackObject { group_id, forwarding, object_id, extension_headers, payload }`
  in arrival order, from subgroup streams and datagrams alike. Subgroup
  streams are read **concurrently**: one task takes every new stream or
  datagram from the receiver and runs one reader future per stream; readers
  and datagrams feed a bounded channel (64 objects) so a slow consumer
  applies QUIC flow control to the publisher instead of buffering without
  bound. Ordering is therefore only guaranteed within a subgroup (draft-14
  §10.4); consumers that need cross-subgroup order sort by the ids themselves.
  It reads the resolved objects `StreamDataReceiver` returns (below);
  status objects are skipped, and `Ok(None)` means the subscription ended. A failure inside one stream (a publisher that leaves resets its
  open group, a GOAWAY cutover cuts the group still open on the retired
  connection) is logged at DEBUG and ends only that stream, so the only `Err`
  is a `MalformedTrackError`, after which the track ends.
- `Subscriber::subscribe_current_group` is how a late joiner reads a track
  from the start of its current group (draft-14 §9.16.2, the catalog join of
  draft-ietf-moq-msf-01 §5). It subscribes with Largest Object and writes a
  Relative Joining FETCH (offset 0) right behind the SUBSCRIBE on the same
  connection, before SUBSCRIBE_OK, so both are answered within one round
  trip. The peer handles its requests in order, so the FETCH finds the
  subscription it joins. It returns a `TrackReader` whose task drains the
  FETCH into the channel, then reads the subgroup streams and datagrams, so the fetched
  objects come before the live ones even when a live stream arrives first.
  Until the FETCH ends, the live streams wait in the subscription's channel.
  For a track without content the peer refuses the FETCH with INVALID_RANGE
  (§9.16.2); a refused or failed FETCH is logged at DEBUG, and the reader
  yields only the live objects.
  `Origin` answers the FETCH, so this works against a moqt publisher as well as
  a relay.
- `Subscriber::subscribe_group_start(namespace, name, new_group_after)` is
  the late join of a track whose groups are long, such as video with sparse
  keyframes (draft-18 §5.1.3.1). It subscribes with Largest Object, waits
  for the SUBSCRIBE_OK and sends one request more, chosen by what the
  SUBSCRIBE_OK tells, so it sends no request it withdraws:
  - The track has no DYNAMIC_GROUPS (§12.6), or its Largest Object ID is
    below `new_group_after`: it sends the Relative Joining FETCH (offset 0)
    and the `TrackReader` yields the current group from its start, then the
    live objects. A track without content has no group to fetch and is read
    live.
  - Otherwise it sends `request_new_group` and the `TrackReader` yields from
    the first group after the Largest Group. The rest of the Largest Group
    cannot be decoded without its start, so the reader holds it back. When
    no newer group arrives within `NEW_GROUP_WAIT` (1 s), it sends the
    Joining FETCH for the Largest Group and yields the fetched objects, then the held ones, then
    the live ones. A publisher may hold the request back or have no frame
    to encode, so the wait has to end.
- `Subscriber::request_new_group(&subscription, largest_group)` sends a
  REQUEST_UPDATE that carries only NEW_GROUP_REQUEST (§10.2.13: the largest
  group the caller knows plus 1, or 0), on the stream of the SUBSCRIBE or of
  the peer's PUBLISH. It returns `false` without sending for a track without
  DYNAMIC_GROUPS, which §10.2.13 forbids asking, so it never sends on
  draft-14. `SubscribeOption::new_group_request` puts the parameter in a
  SUBSCRIBE, which needs no foreknowledge; it is not carried over to the
  SUBSCRIBE a session sends again after GOAWAY. NEW_GROUP_REQUEST in
  PUBLISH_OK is neither sent nor read.

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
  group after the largest one written when PUBLISH_OK arrives; it is a no-op
  while the session takes the track and after it released a PUBLISH, but
  publishes a track again whose SUBSCRIBE the peer released. The origin state
  is not locked while PUBLISH_OK is awaited, so writes and requests on every
  track go on. A second lock, `publishing`, makes `publish` calls run one at a
  time across all tracks and is always taken before the state lock. The peer
  can UNSUBSCRIBE right after its PUBLISH_OK, before `publish` registers the
  request, so `release` of an unknown request first waits on `publishing`;
  `accept_subscribe`
  answers a SUBSCRIBE the application decided to accept (Content Exists is the
  largest cached location, the last one FETCH can serve) and writes from its filter's Start Location
  (draft-14 §9.7), computed from the largest object written so far. Objects
  before the Start Location are skipped; when the first object at or after it
  does not start a group, the writer opens that group on a new stream starting
  at that object, so Largest Object takes the rest of the open group;
  `release(&handler)`
  drops the session's writer on UNSUBSCRIBE, resetting its open group with
  CANCELLED rather than finishing it (draft-14 §10.4.3), and returns the
  track so the caller can account it; `serve_fetch` answers a Standalone or
  Joining FETCH from the cache on its own stream (`FetchServeTask`, one per
  request, aborted by `cancel_fetch` and when the `Origin` is dropped). Each
  accepted SUBSCRIBE keeps its `JoinPoint`: the Largest Location its
  SUBSCRIBE_OK reported, or that its filter is not Largest Object. A Joining
  FETCH ends right after that location (draft-14 §9.16.2.1), so it meets the
  live objects without overlap. It is rejected with INVALID_JOINING_REQUEST_ID
  once the SUBSCRIBE is released, with INVALID_RANGE when the SUBSCRIBE_OK
  reported no content, and closes the session with PROTOCOL_VIOLATION when the
  filter is not Largest Object (`FetchHandler::close_for_protocol_violation`);
  `answer_track_status` fills TRACK_STATUS_OK like a SUBSCRIBE_OK.
- Each outlet keeps its Forward State, from its SUBSCRIBE or PUBLISH_OK and
  from every SUBSCRIBE_UPDATE / REQUEST_UPDATE `handle` sees, and `write`
  skips an outlet in Forward State 0 (draft-18 §5.1). An outlet that skipped
  an object of its open group leaves that group (`TrackWriter::abandon_group`)
  and, once back in Forward State 1, takes it up again on a new stream from
  the next object written (§11.4.3); the left stream is reset, not finished.
  `publish_on_demand(namespace, name, demand_sender)` makes the track's PUBLISH
  ask for Forward State 0, so the peer has it sent only while it wants the
  objects (§10.10), and sends a `TrackDemand` — whether any outlet of the
  track is in Forward State 1 — when called and at every change, so the
  application can stop producing a track nobody takes.
- `end_track(namespace, name)` ends every outlet of the track with
  PUBLISH_DONE (TRACK_ENDED, draft-14 §9.12) and keeps its numbering and
  cache, so a later `publish` or SUBSCRIBE takes it up again at the next
  group without reusing a location. Each outlet's open group is finished with
  End of Group before PUBLISH_DONE, which may not precede the close of any
  stream of the subscription, and its Stream Count is the number of streams
  the outlet's `TrackWriter` opened. An outlet whose writer a write holds is
  marked ending and ended when the write puts the writer back.
- `accept_new_group_requests(namespace, name, requested)` gives a track
  dynamic groups (draft-18 §12.6): its PUBLISH and SUBSCRIBE_OK carry
  DYNAMIC_GROUPS, and the `Notify` is notified when a SUBSCRIBE or a
  REQUEST_UPDATE carries a NEW_GROUP_REQUEST of 0 or of a group beyond the
  one being written (§10.2.13); a request for a group that already began is
  dropped. The application starts the new group with the next object it
  writes. `origin/new_group_requests.rs` keeps a group from getting shorter
  than `MIN_GROUP_DURATION` (2 s) and `MIN_GROUP_OBJECTS` (60), the delay
  §10.2.13 allows an Original Publisher: a request is notified at once when
  the open group is that old or holds that many objects, and otherwise by
  `NewGroupDelayTask` as soon as it is either. A subscriber that joins a
  shorter group fetches it, so only a request made for another reason (a
  rendition switch, a decoder that lost its group) waits. A request is
  notified once per open group: those that follow it before the next group
  begins add nothing, and a group the application starts on its own answers
  a request still held back.
- `handle(event)` is the session event loop's shortcut to all of the above:
  it accepts a SUBSCRIBE to a registered track, releases on UNSUBSCRIBE,
  serves FETCH, aborts on FETCH_CANCEL and answers TRACK_STATUS, and hands
  back every other event, and a SUBSCRIBE or UNSUBSCRIBE for a track or
  request it does not know, to the caller. A SUBSCRIBE_UPDATE is handed back
  too, after its NEW_GROUP_REQUEST was passed to the track.
- Left to the application: whether to accept a SUBSCRIBE for a track it
  has not registered yet (`handle` returns it), namespace
  announcements, when to PUBLISH, retention and first group ids, and
  disconnect handling. The FETCH range arithmetic is the free functions
  `resolve_fetch_range` (the Standalone End Location) and
  `resolve_joining_fetch` (a Joining Fetch's Start and End Location and its
  `JoiningFetchError`) in `domains/fetch_range.rs`, shared with the relay and
  the wasm bindings.

### `NamespaceWatchers` — the namespaces a peer publishes

`namespace::NamespaceWatchers` (`domains/namespace_watchers.rs`) is the
subscribing side's counterpart of `Origin` for namespaces: `watch(prefix)`
returns a channel of `NamespaceEvent`s, and `handle(event)` in the session
event loop answers PUBLISH_NAMESPACE with `ok`, sends it and
PUBLISH_NAMESPACE_DONE to every watcher of a matching prefix, ends every
watcher on `Disconnected` and hands every other event back. A watcher is
registered before its SUBSCRIBE_NAMESPACE is sent, since the peer publishes
the namespaces it knows right away. The Python bindings and the browser SDK's
`Session` (`crates/wasm/src/session.rs`), which hand out one iterator per
namespace subscription, use it.

### `SessionContext` and `Connection` — shared session state

`Connection` (`domains/connection.rs`) holds what draft-14 scopes to one
transport connection:

- `transport` — the QUIC, WebTransport or WebSocket connection.
- `control: ControlChannel` — the version adapter above; for draft-14 it
  holds this side's next request id, the peer's next request id and
  `inflight_requests`.
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
- `object_sinks: ObjectSinks` — the sink of each track alias: the objects arriving on
  this connection, keyed by wire alias; a sink sets the logical alias on
  every stream it hands out; see buffering invariant below.
- `fetch_notification_map` — keyed by request id; `FetchNotifier` removes the
  sender when the FETCH stream arrives, since a FETCH is answered on a single
  stream (draft-14 §9.16.3). A stream that finds no sender is stopped with
  STOP_SENDING (CANCELLED). This is expected when this side cancelled the
  FETCH before the stream arrived; only a stream for a request id this side
  never wrote is an error.

`SessionContext` holds the rest of the cross-task state, the current
`Connection` and, during a migration, the previous one; `connection()` is the
connection requests are sent on, and the methods that touch per-connection
state name the connection:

- `event_sender`.
- `receiver_map` and `fetch_receiver_map` — the receivers the application
  takes per logical track alias and per logical FETCH request id. A
  subscription's entry keeps the version of the connection it was accepted
  on, which decides whether its track may mix forwarding preferences; a
  FETCH's entry names its track, whose `ObjectLedger` the receiver joins
  when the application takes it. Registering a FETCH returns
  a `PendingFetch` drop-guard that removes both FETCH entries unless FETCH_OK
  arrived.
- `ledgers` — a `Weak<ObjectLedger>` per full track name. `object_ledger`
  hands the live one to every subscription and FETCH of the track it
  receives, or starts a new one; the receivers hold it, so it ends with the
  last of them, and a migration keeps it. A FETCH receiver lets go of it
  when the response ends (FIN, or an error the session cannot carry over),
  and a response the application never takes holds none.
- `carried` — for a session that follows GOAWAY, the namespace (with their
  `NamespaceInterest`) and track subscriptions and the published namespaces a migration sends again. A
  request takes its read lock before it is sent and holds it until it is
  recorded or fails (through the answer for SUBSCRIBE and PUBLISH), editing
  the record through an inner mutex, so concurrent requests do not wait for
  each other's answers. A migration holds the write lock while it sends them
  again, so none is missed.
- `publications` — for a session that follows GOAWAY, the tracks PUBLISHed
  on it with every connection each is on (`SendTargets::Publication`); a
  track served for a SUBSCRIBE goes to the connection the SUBSCRIBE arrived
  on (`SendTargets::Connection`). A PUBLISH holds the `carried` read lock
  until it is added here, so a migration does not miss it.

### Request spans

Each request a client sends and waits on has one INFO span, from writing the
request to receiving its response. On a GOAWAY migration the span covers every
attempt; it adds no field for the retry.

| Span | Fields |
| --- | --- |
| `moqt.subscriber.subscribe` | `track_namespace`, `track_name`, `subscriber_priority`, `forward`, `request_id`, `outcome` |
| `moqt.subscriber.subscribe_namespace` | `track_namespace_prefix`, `request_id`, `outcome` |
| `moqt.subscriber.fetch` | `track_namespace`, `track_name`, `request_id`, `outcome` |
| `moqt.publisher.publish` | `track_namespace`, `track_name`, `request_id`, `outcome` |
| `moqt.publisher.publish_namespace` | `track_namespace`, `request_id`, `outcome` |
| `moqt.session.connect` | `url`, `outcome` |

- `request_id` is the wire Request ID of the last attempt, the one the
  `*_OK received` / `*_ERROR received` logs print; those logs carry
  `duration_ms`, the time from writing the request to its response.
- `outcome` is `ok` when the `*_OK` arrived, `rejected` when the peer answered
  `*_ERROR`, and `failed` for any other error (transport, protocol violation,
  session closed).
- `begin_publish_namespace` returns before the response and has no span; the
  other `Subscriber` / `Publisher` spans and `moqt.session` stay at DEBUG.
- `moqt.session.connect` is one connection attempt of a client endpoint
  (`Endpoint::connect`, and each GOAWAY reconnect): opening the transport,
  the authorization token provider, and CLIENT_SETUP until SERVER_SETUP.
  `outcome` is `rejected` when the server closed the connection before
  SERVER_SETUP. It roots its own trace and follows the caller's span, which
  may live as long as the session. The transport is opened outside it,
  because quinn runs a connection's driver in the span current at connect,
  and it closes before the session's tasks start, so neither holds it open.

The five requests carry the span's W3C `traceparent` in a `TRACE_CONTEXT`
parameter (type `0x1fff`, not in draft-14), so the receiver's span joins the
sender's trace. Draft-14 §9.2 makes a receiver ignore an unknown parameter,
but in draft-18 an unknown Message Parameter is a PROTOCOL_VIOLATION (§10.2)
and an extension is agreed through a Setup Option (§3.2). So both CLIENT_SETUP and
SERVER_SETUP always carry a `TRACE_CONTEXT` Setup Option (type `0x1fff`, an
empty value), and a connection sends the parameter only when the peer's SETUP
carried it (`ControlChannel::peer_reads_trace_context`). In CLIENT_SETUP the
option's value is the traceparent of `moqt.session.connect` (read before the
server's SETUP is known; a server that does not know the option ignores it,
draft-14 §9.3), and `Handshake::trace_context()` returns it. Draft-18 negotiates
the same extension: its SETUP carries the `TRACE_CONTEXT` option under the
same type, since Setup Options share one namespace across versions (§10.3),
and the five requests and SUBSCRIBE_TRACKS carry the Message Parameter
`0x1fff` as length-prefixed bytes. A draft-18 receiver parses the parameter in
those messages without checking either SETUP, so receiving it never closes
the session. Setup Options have no
private range, so publishing the extension would need a provisional
registration.
`moqt` has no OpenTelemetry dependency: the application gives it the current
span's `traceparent` with `set_trace_context_provider` (the relay through
`telemetry::current_traceparent`, the wasm module and `moq-cli` through
`client_telemetry::current_traceparent`), and the send functions read it inside the request span. Without a
provider the parameter is omitted. On receipt, `trace_context()` on
`SubscribeHandler`, `PublishHandler`, `FetchHandler`,
`SubscribeNamespaceHandler` and `PublishNamespaceHandler` returns the
`TraceContext`; a value that is not a traceparent counts as absent.

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
- `TrackProperties` is what PUBLISH, SUBSCRIBE_OK, TRACK_STATUS_OK and
  FETCH_OK say about the track. Received values are on `Subscription::track_properties()` (from
  SUBSCRIBE_OK or PUBLISH), `PublishHandler.track_properties` and
  `FetchHandle.track_properties`; values to send go in
  `SubscribeHandler.track_properties`, `FetchHandler.track_properties` and
  `PublishOption.track_properties`.
  - The property list is the single source: `max_cache_duration`,
    `delivery_timeout`, `group_order` (DEFAULT PUBLISHER GROUP ORDER) and
    `default_publisher_priority` read it, in the mutable list first and then
    inside Immutable Properties (§12.7), and each setter replaces only the
    entries of its type. No message, handler or option keeps a copy of these
    values; `SubscribeOk.group_order` is the order a subscription is
    delivered in, which only the draft-14 message states.
  - Draft-14 carries the same values as parameters (DELIVERY_TIMEOUT,
    MAX_CACHE_DURATION) and as the Group Order of PUBLISH and SUBSCRIBE_OK;
    its codec maps them to and from the list and sends nothing else, so a
    set from a draft-18 peer loses its other properties toward a draft-14
    one.
  - MAX_CACHE_DURATION (draft-14 §9.2.1.3, milliseconds) is read and sent but
    never enforced here; caching endpoints such as the relay apply it.
  - A draft-14 peer understands no Mandatory Track Property
    (`Version::understands`), so such a track is never sent to it (draft-18
    §2.5.1): `SubscribeHandler::ok*` and `FetchHandler::ok` refuse the
    request with UNSUPPORTED_EXTENSION, which draft-14 sends as NOT_SUPPORTED,
    and fail; `SubscribeHandler::understands` tells beforehand whether `ok*`
    would refuse; `Publisher::publish` fails with
    `RequestFailure::TrackNotUnderstood` and sends nothing. A draft-18 peer
    is sent the properties as they are.
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
  - `ResponseGuard` is the one place that sends a handler's answer: the
    `*_OK` of its `ok()`, and the `*_ERROR` of both its `error()` and the
    drop-time NOT_SUPPORTED reply. It logs each as `*_OK sent` /
    `*_ERROR sent` with the request's `RequestSummary` (kind, wire Request
    ID, track), the same summary `ControlChannel::send_request` logs as
    `* sent`.

The draft-14 `ControlMessageReceiveTask` splits every decoded message into
one of three paths:

1. **Requests** (SUBSCRIBE, PUBLISH, FETCH, namespace messages, …) become
   `SessionEvent` variants delivered to `Session::receive_event()`.
2. **Responses** (`*_OK` / `*_ERROR`) are matched against `inflight_requests` by
   request id and complete the pending `oneshot`. The waiting side holds a
   `PendingResponse` whose drop marks the entry abandoned, so a late response
   runs its `LateResponseAction` instead of closing the session. The
   response itself arrives as a `ReceivedResponse`, which runs the same
   action when dropped before the caller `accept`s it, whether still in the
   `oneshot` or held while the caller registers what it established
   (SUBSCRIBE's receiver). A request future is therefore cancel-safe at any
   point: dropped after the success response, it still sends UNSUBSCRIBE,
   FETCH_CANCEL or PUBLISH_DONE. When the
   control stream closes, every entry is failed and later registrations fail
   at once.
3. **Protocol violations** (a malformed payload, a SETUP after the handshake,
   a response to a request kind this crate never sends) close the session and
   stop the task.

Messages `Session` decodes but does not act on (GOAWAY, unless the session
follows it, SUBSCRIBE_UPDATE, PUBLISH_DONE, FETCH_CANCEL,
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
- `data_receiver.rs` — `DataReceiver`, what `accept_data_receiver` returns at
  once for a subscription: `next()` yields `TrackData::Subgroup(StreamDataReceiver)`
  or `TrackData::Datagram(Object)` in arrival order. A draft-14 track that
  mixes the two fails with `MalformedTrackError::MixedForwardingPreference`
  (draft-14 §2.5, §10); draft-18 allows the mix (§11).
- `immutable_properties.rs` — the object rule of draft-14 §11.2 and draft-18
  §12.7: Immutable Properties that hold Immutable Properties, or a
  Key-Value-Pair that does not parse in the form of the connection's draft,
  are `MalformedTrackError::MalformedImmutableProperties`. Subgroup streams
  (at an object's head), datagrams and FETCH responses check every object
  before the ledger does.
- `object_ledger.rs` — `ObjectLedger`, the duplicate rule of draft-14 §8.1 and
  draft-18 §9.1 for one track: per location it keeps the Subgroup ID (or a
  datagram mark) and one word packing the priority, the status, the
  draft-14 FETCH mark and a 52-bit hash of the payload. An entry is 32 B
  with its `Location` key, about 38 B with hashbrown's control byte at its
  7/8 load. No entry is removed while a receiver of the track lives, so a
  long-lived subscription at 80 objects/s holds about 260 MB a day. A copy
  that differs in preference, subgroup, priority or payload is
  `MalformedTrackError::ConflictingDuplicate`, and so is, on draft-14
  (§8.1), one whose status moves between Normal, End of Group and End of
  Track; draft-18 §9.1 compares no status. The draft is that of the
  connection the copy came on (`CopySource`). Extension headers are not
  compared and a move to or from Does Not Exist passes. A draft-14 FETCH
  carries no preference, so its copy in the subgroup of its Object ID matches
  a datagram until a copy with a known preference arrives. Its
  `std::sync::Mutex` is never held across an await.
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
    `StreamReceiveError::ProtocolViolation`; an object the ledger rejects is
    `StreamReceiveError::MalformedTrack` and is not returned.
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
    error. The ledger hashes an arriving payload piece by piece and checks
    the object with its last piece, so a rejected object ends with
    `StreamReceiveError::MalformedTrack` in place of that piece.
  - `DatagramSender::send_object(&Object)` / `TrackData::Datagram(Object)`
    do the same for datagrams (`ObjectDatagram::from_object` /
    `into_object`): an object id is always sent explicitly, a zero-length
    object is sent as its status (§10.2.1.1), and an object with a payload is
    always `Normal`: an End of Group type sets `Object::ends_group` instead of
    the status, and only a datagram with a payload sends it back. FETCH cannot
    carry it, so a fetched copy of that object has `ends_group` false. A
    datagram larger than the session's maximum datagram size is dropped and
    the send succeeds (draft-14 §10.3, draft-18 §11.3); the QUIC and
    WebTransport connections map quinn's `TooLarge` to that drop.
  - `Publisher::mixes_forwarding_preferences()` tells, without naming the
    draft, whether one track sent on the session may use subgroup streams and
    datagrams alike (draft-18 §11) or keeps one preference (draft-14 §10).
  - `FetchDataSender::send_object(&Object)` / `FetchDataReceiver::receive()
    -> Fetch::{Object(Object), UnknownRange { last }, End}` do the same for
    FETCH responses (`FetchObjectField::from_object` / `into_object`): a
    datagram object is sent with its Object ID as the Subgroup ID (§10.4.4)
    and a zero-length object as its status. The FETCH_HEADER is consumed by
    moqt and not returned. `UnknownRange` is draft-18's End of Unknown Range
    (§11.4.4.2): the objects through `last` were not sent and may exist, so
    `End` after it does not mean the range is complete. draft-14 never
    yields it; draft-18's End of Non-Existent Range yields nothing. Each
    object is checked against its track's ledger (a conflict fails with
    `MalformedTrackError`); `carries_forwarding_preference()` is `false` on
    draft-14, whose datagram objects arrive in the subgroup of their Object
    ID.
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
  - `DataReceiver` / `StreamDataReceiver` and `FetchDataReceiver` mirror the
    senders on the other side.

## Runtime dispatch (`modules/moqt/runtime/dispatch`)

- `SubscriptionNotifier` routes incoming objects by track alias into
  `Connection::object_sinks`.
- `FetchNotifier` routes FETCH streams by request id as a
  `FetchDataReceiver`.
- `IncomingObject` is the per-track-alias envelope (`Subgroup` holding a
  `StreamDataReceiver` / `Datagram` holding an `Object`); FETCH streams never
  enter the per-track channel.

## Key invariants

- **Request IDs reach the peer in order and without gaps** (draft-14 §9.1). A
  `RequestIdReservation` holds the next Request ID's lock until the request
  is written, so concurrent requests cannot overtake each other on the
  control stream. The next Request ID advances only once the request is
  written, so dropping the reservation or the request future before then
  leaves the ID for the next request.
  `ControlMessageReceiveTask` closes the session with INVALID_REQUEST_ID
  unless a peer's new request uses exactly its previous Request ID plus 2
  (for its first, 0 from a client and 1 from a server).

- **Request IDs stay below the peer's Maximum Request ID** (draft-14 §9.5).
  `reserve_request_id` waits while the next ID reaches the limit from the
  peer's SETUP or MAX_REQUEST_ID, sending one REQUESTS_BLOCKED per limit. A
  MAX_REQUEST_ID that does not raise the limit closes the session with
  PROTOCOL_VIOLATION. A received request at or above the advertised limit
  closes it with TOO_MANY_REQUESTS. The advertised limit follows
  use, not completion: once the peer has used half of the 1000-ID window,
  `ControlMessageReceiveTask` sends MAX_REQUEST_ID for a full window past the
  peer's next ID, so a long-lived session never runs out and the number of
  concurrent requests is not capped. MAX_REQUEST_ID and REQUESTS_BLOCKED never
  reach the application.

- **A peer cannot declare an object payload or an extension header block
  above 16 MiB** (`MAX_WIRE_DECLARED_LENGTH`). A larger Object Payload
  Length or Extension Headers Length (draft-18: Properties Length) on a
  subgroup or FETCH stream is a decode error that ends the stream (draft-14
  §12.1, draft-18 §13.5). Neither draft sets a limit, and a receiver can
  hold a declared length in memory until all of it arrives.

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
  `connection.rs`), dropping the oldest on overflow, and at most 1024 objects
  across a connection's unknown aliases (`MAX_PENDING_OBJECTS`), dropping a
  later object. A buffer expires 10 s after its first object
  (`UNKNOWN_TRACK_ALIAS_PERIOD`, draft-14 §10.3 "a brief period"), which
  leaves an application time to accept a PUBLISH.
  `register_data_receiver` drains the buffer into the new channel atomically
  under the sinks lock.
- **Unread datagrams are bounded**: a subscription's channel queues at most
  1024 datagrams its application has not read (`MAX_QUEUED_DATAGRAMS` in
  `incoming_object.rs`) and drops a later one, since datagrams take no flow
  control. Subgroup streams are not counted: an unread stream holds its QUIC
  stream credit, so the peer's stream limit bounds them.
- **Cancelled subscriptions discard late objects** (draft-14 §10.1):
  `Subscriber::unsubscribe`, the UNSUBSCRIBE sent for a late SUBSCRIBE_OK and
  `PublishHandler::error` (a PUBLISH with Forward=1 sends objects before its
  answer, §9.13) turn the track alias's sink into `Cancelled`, which drops
  the buffered objects, the channel sender and a receiver not yet taken by
  `accept_data_receiver`. Objects still in flight are then dropped on arrival
  instead of being buffered as an unknown alias. The `Cancelled` marker stays
  until a new subscription reuses the alias or `UNKNOWN_TRACK_ALIAS_PERIOD`
  (10 s) ends; an object arriving later is buffered as one of an unknown
  alias, bounded and expiring like any other. It is not released on
  PUBLISH_DONE because late streams may follow it (§9.12).
- **PUBLISH_DONE ends the subscription once its streams have arrived**
  (draft-14 §9.12): the event still reaches the application, and
  `PublishDoneExpiryTask` cancels the subscription as an unsubscribe would,
  without sending UNSUBSCRIBE, once the sink has taken Stream Count subgroup
  streams. When they do not all arrive (a Stream Count of 2^62-1, a stream
  reset before its header) it cancels 10 s after PUBLISH_DONE instead (the
  draft's SHOULD for a timeout). A Stream Count of 0 waits the 10 s too: a
  datagram track sends 0 and its datagrams can still follow PUBLISH_DONE. The data receiver then ends, and
  `TrackReader` returns `Ok(None)` once the streams it already accepted are
  drained. A stream beyond the Stream Count is not a PROTOCOL_VIOLATION (a
  MAY not taken), and no STOP_SENDING is sent. A second PUBLISH_DONE for a
  subscription that is still waiting is ignored.
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
- **PUBLISH_DONE for a Request ID that never existed** (draft-14 §9.1): the
  same check closes the session with `InvalidRequestId` before
  `PublishDoneExpiryTask` sees it. A Request ID that existed but names no
  subscription still gets the wait, and its cancel finds nothing: the sink of
  a subscription may be installed after its PUBLISH_DONE arrives.
- **Control response timeout**: `PendingResponse::response` bounds every
  request/response wait to 10 s; a timeout fails only that request with
  `RequestFailure::Timeout` and keeps the session open (draft-14 §12.2).
  On draft-14 the request's `inflight_requests` entry stays as `Abandoned`
  until a late response or the control stream's end, and abandoning more
  than 100 of them closes the session with CONTROL_MESSAGE_TIMEOUT.
  Every request that waits for a response returns `RequestFailure`, so
  callers tell a peer rejection, a timeout and a closed session apart by
  matching instead of downcasting.
- **One forwarding preference per draft-14 track** (draft-14 §2.5, §10): the
  first data of a draft-14 subscription fixes subgroup streams or datagrams,
  and data of the other kind makes `DataReceiver::next` return
  `MalformedTrackError::MixedForwardingPreference`, which `TrackReader` passes
  on. A draft-18 track may carry both (§11). moqt does not UNSUBSCRIBE on its
  own: the caller does, because a relay must also end its downstream
  subscriptions.
- **A duplicate object must match** (draft-14 §8.1, draft-18 §9.1): every
  object a subscription or FETCH of a track delivers is checked against the
  track's one `ObjectLedger`, whichever connection of the session it came
  on, so a conflicting copy is reported as
  `MalformedTrackError::ConflictingDuplicate` instead of being returned.
  Identical copies are returned; moqt does not drop duplicates.
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
  await `Session::close()` (or `Endpoint::wait_idle()` after the last handle
  is gone) before exiting, or the peer only notices through its idle timeout.
  A client session keeps the endpoint it was opened on. `close` therefore
  closes the connection with NO_ERROR even while other handles are alive, then
  waits for that endpoint to be idle. The application need not keep the
  `Endpoint`.
- **No direct runtime calls in the session stack**: `modules/moqt` spawns,
  yields, bounds waits and reads the clock only through `modules/executor`
  (`spawn`, `try_spawn`, `yield_now`, `timeout`, `now`, `system_time`); `tokio::sync`
  channels and locks are used directly. `modules/transport` is the only other
  place that touches tokio I/O. `executor/native.rs` is the tokio
  implementation (`now` is the elapsed time since a process-wide `Instant`),
  `executor/browser.rs` the `spawn_local` one (`now` reads
  `performance.now()`; `std::time::Instant` panics on wasm32, and
  `set_spawn_wrapper` lets the application wrap every task where it is
  spawned, which the wasm module uses to carry a session's telemetry scope
  into its tasks); the executor also defines
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
  draft-14 only.
- **Closing a send stream does not wait for the peer to acknowledge the FIN**
  (`TransportSendStream::close`). quinn and WebTransport call `finish()`; the
  browser transport does not await `writer.close()`. Awaiting the ACK stalls
  every send queued behind a stream close. No test guards the browser
  transport's close.

## Testing conventions
Unit tests live in `#[cfg(test)] mod tests` inside the module under test
(Arrange/Act/Assert, see `wire.rs` framing round-trip tests). Cross-crate
behaviour is covered by the workspace-level E2E suites under `tests/`.

# `tokio-tungstenite` for QMux over WebSocket (`moqt`)

**Status:** Accepted (2026-10-28)

## What

Add `tokio-tungstenite` as a native-only dependency of `moqt` (`default-features = false`, feature: `handshake`). `modules/transport/websocket` uses it for:

- Opening handshakes (`accept_hdr_async_with_config` on the relay, `client_async_with_config` on clients).
- RFC 6455 message framing beneath a QMux session.

## Context

QMux over WebSocket (`draft-lcurley-qmux-websocket-00`) is the fallback for clients that cannot use QUIC. The relay accepts TLS WebSocket upgrades on TCP, negotiates the QMux subprotocol via `Sec-WebSocket-Protocol`, and exchanges binary, Ping/Pong, and Close frames.

The native client needs the counterpart for `wss://` URLs and tests. Because `moqt` already handles TLS via `rustls`/`tokio-rustls`, the WebSocket crate only needs to run over an `AsyncRead + AsyncWrite` stream.

## Alternatives

- **`hyper` + `hyper-tungstenite`:** `hyper` is only in the dependency tree as a client (via `reqwest`); serving upgrades adds the `hyper` server, `hyper-util`, and a second framing crate even though the relay serves no other HTTP.
- **`fastwebsockets`:** Lower framing overhead, but handshake helpers assume `hyper` and the crate is less widely adopted.
- **Hand-written RFC 6455:** Masking, fragmentation, control frames, and client/server handshakes match the crate's scope without its battle-testing.

## Decision

Use `tokio-tungstenite` with only the `handshake` feature enabled (leaving `connect` and TLS features off). `moqt` opens TCP and wraps TLS via `tokio-rustls` using existing `client_crypto.rs` and `server_transport_config.rs` configs, adding only `tungstenite`, `httparse`, `sha1`, and `data-encoding`.

The split `WebSocketStream` implements QMux's `MessageSink` and `MessageSource`. The WebSocket message size limit is set above `max_record_size` so oversized records trigger QMux's `FRAME_ENCODING_ERROR` instead of a socket error.

# tokio-tungstenite for QMux over WebSocket

## Status

Accepted

## Date

2026-10-28

## What

Add `tokio-tungstenite` as a native-only dependency of the `moqt` crate,
with `default-features = false` and only the `handshake` feature.
`modules/transport/websocket` uses it for the WebSocket opening handshake
(`accept_hdr_async_with_config` on the relay, `client_async_with_config` on
the client) and for the message framing under a QMux session.

## Context

QMux over WebSocket (draft-lcurley-qmux-websocket-00) is the fallback for
clients that cannot use QUIC. The relay must accept a WebSocket upgrade over
TLS on its TCP port, select the QMux subprotocol from `Sec-WebSocket-Protocol`,
and then exchange binary messages, Ping/Pong and Close frames per RFC 6455.
The native client needs the same on the connecting side, for the tests and
for `wss://` URLs. TLS is already provided by rustls through quinn, so the
WebSocket crate only has to run on an existing `AsyncRead + AsyncWrite`
stream.

## Alternatives

- `hyper` + `hyper-tungstenite`. `hyper` is in the dependency tree through
  `reqwest`, but only as a client; serving the upgrade would add the hyper
  server, `hyper-util` and a second crate for the WebSocket framing on top.
  The relay serves no other HTTP, so an HTTP server is not needed.
- `fastwebsockets`. Lower overhead framing, but its handshake helpers are
  built around hyper, and it is less widely used.
- Hand-written RFC 6455. The server side is small, but masking,
  fragmentation, control frames and the client side together are the size of
  the crate, with none of its testing.

## Decision

Use `tokio-tungstenite` with `default-features = false, features = ["handshake"]`.
The `connect` feature and the TLS features stay off: the connection is
opened and wrapped in TLS by `moqt` itself (`tokio-rustls` with the rustls
configs `client_crypto.rs` and `server_transport_config.rs` already build),
so the crate adds only `tungstenite`, `httparse`, `sha1` and `data-encoding`.
The `WebSocketStream` is split into a sink and a stream, which implement
QMux's `MessageSink` and `MessageSource`; the message size limit is set
above `max_record_size` so that an oversized record is answered by QMux's
FRAME_ENCODING_ERROR rather than by a socket error.

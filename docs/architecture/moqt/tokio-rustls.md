# tokio-rustls for the WebSocket TLS connection

## Status

Accepted

## Date

2026-10-28

## What

Add `tokio-rustls` as a native-only dependency of the `moqt` crate, with
`default-features = false` and the `ring`, `logging` and `tls12` features.
`modules/transport/websocket` uses its `TlsAcceptor` on the relay's TCP
listener and its `TlsConnector` in the `wss://` client.

## Context

QMux over WebSocket runs on TCP, where TLS is not part of the transport as
it is in QUIC. The relay already holds a rustls `ServerConfig` built from its
certificate for quinn, and the client already builds a rustls `ClientConfig`
(native roots, skip-verify for tests, or a custom CA). What is missing is
the async TLS stream on a `TcpStream` that `tokio-tungstenite` can run the
handshake on.

## Alternatives

- `tokio-native-tls` / `native-tls`. A second TLS stack next to rustls, with
  platform-specific behaviour and no way to reuse the existing configs or
  the skip-verify verifier the tests rely on.
- `tokio-tungstenite`'s `rustls-tls-*` features. They pull `tokio-rustls` in
  anyway, plus a root store the crate chooses, and only cover the client.

## Decision

Use `tokio-rustls` directly. It is the async layer for the `rustls` version
quinn already depends on (0.23), so the `ServerConfig` and `ClientConfig`
types are shared and `install_default_crypto_provider` (ring) covers both.
The default `aws_lc_rs` feature is turned off so that no second crypto
provider enters the build. The server config sets ALPN `http/1.1`, which the
WebSocket upgrade runs over; the client config for WebSocket is the QUIC
client config with that ALPN.

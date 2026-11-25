# `tokio-rustls` for WebSocket TLS (`moqt`)

**Status:** Accepted (2026-10-28)

## What

Add `tokio-rustls` as a native-only dependency of `moqt` (`default-features = false`, features: `aws_lc_rs`, `logging`, `tls12`). `modules/transport/websocket` uses `TlsAcceptor` on the relay's TCP listener and `TlsConnector` in the `wss://` client.

## Context

QMux over WebSocket runs over TCP, where TLS is not built into the transport as in QUIC. The relay already builds a `rustls::ServerConfig` for `quinn`, and the client already builds a `rustls::ClientConfig` (native roots, test skip-verify, or custom CA). We need an async TLS wrapper over `TcpStream` that `tokio-tungstenite` can handshake on.

## Alternatives

- **`tokio-native-tls` / `native-tls`:** Adds a second TLS stack with platform-specific behavior and cannot reuse existing `rustls` configs or the test skip-verify verifier.
- **`tokio-tungstenite`'s `rustls-tls-*` features:** Pulls in `tokio-rustls` anyway along with its own root store, and only covers the client side.

## Decision

Use `tokio-rustls` directly, matching `quinn`'s `rustls` 0.23 version so `ServerConfig`, `ClientConfig`, and `install_default_crypto_provider` (`aws-lc-rs`, see [aws-lc-rs.md](aws-lc-rs.md)) are shared. Both server and client WebSocket TLS configs set ALPN to `http/1.1` for the WebSocket upgrade.

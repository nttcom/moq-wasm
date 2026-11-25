# `aws-lc-rs` as the TLS Crypto Provider (`moqt`)

**Status:** Accepted (2026-11-10)

## What

Run every `rustls` connection on the `aws-lc-rs` crypto provider:

- Enable `aws-lc-rs` on `quinn`, `web-transport-quinn`, and `tokio-rustls`.
- `install_default_crypto_provider` installs `rustls::crypto::aws_lc_rs`, and `rcgen` signs self-signed certificates with it.
- `moqt` adds `rustls` as a direct dependency solely to enable `prefer-post-quantum`; `telemetry-otlp` does the same for services that do not link `moqt`.

## Context

Recorded TLS 1.3 handshakes can later be decrypted once quantum computers break X25519 key exchange. Hybrid `X25519MLKEM768` closes that gap and is already negotiated by browsers and major servers, but `ring` does not implement ML-KEM.

Previously, the workspace compiled two crypto libraries: `ring` (QUIC, WebSocket, tokens) and `aws-lc-rs` (via `reqwest`, `object_store`, `opentelemetry-otlp`).

Because a `rustls` server selects the key share in client preference order, the client must list `X25519MLKEM768` first. `rustls`'s `aws-lc-rs` provider only does so when `prefer-post-quantum` is enabled—a default feature that is otherwise off because workspace crates disable `rustls` defaults.

## Alternatives

- **Standardize on `ring` and remove `aws-lc-rs` from HTTP clients:** Saves ~70 s of a 3m16s clean build (`aws-lc-sys` on the critical path), but loses post-quantum key exchange everywhere and makes `reqwest` panic if a client is built before installing a provider.
- **Keep both libraries:** Still no post-quantum MoQT, plus two crypto libraries.

## Decision

Use `aws-lc-rs` workspace-wide with `prefer-post-quantum` enabled. MoQT (QUIC and WebSocket) and `reqwest` HTTPS connections offer and accept `X25519MLKEM768`, falling back to `X25519` for peers without it. We accept the `aws-lc-sys` build cost. A test in `crypto_provider.rs` fails if `prefer-post-quantum` is dropped.

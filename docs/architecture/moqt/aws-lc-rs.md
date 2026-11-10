# aws-lc-rs as the TLS crypto provider

## Status

Accepted

## Date

2026-11-10

## What

Run every rustls connection on the `aws-lc-rs` crypto provider: `quinn`,
`web-transport-quinn` and `tokio-rustls` take their `aws-lc-rs` features,
`install_default_crypto_provider` installs `rustls::crypto::aws_lc_rs`, and
`rcgen` signs the self-signed certificate with it. `moqt` also adds `rustls`
as a direct dependency for one feature, `prefer-post-quantum`; `telemetry`
does the same for the services that do not link `moqt`.

## Context

A TLS 1.3 handshake recorded today can be decrypted once a large enough
quantum computer breaks its X25519 key exchange, and meetings carried over
MoQT may need to stay confidential for longer than that. The hybrid
X25519MLKEM768 key exchange closes that gap; browsers and major servers
already negotiate it. `ring` has no ML-KEM, so with `ring` neither MoQT nor
HTTPS can use it.

The workspace also compiled two crypto libraries: `ring` for QUIC, WebSocket
and the token code, and `aws-lc-rs` through `reqwest`, `object_store` and
`opentelemetry-otlp`.

A rustls server picks the key share in the client's order, so a client that
offers X25519MLKEM768 first gets it. rustls' aws-lc-rs provider lists
X25519MLKEM768 first only with the `prefer-post-quantum` feature, which is in
rustls' default features but off here because every crate turns rustls'
defaults off.

## Alternatives

- Keep `ring` and drop `aws-lc-rs` from the HTTP clients. Removes the
  `aws-lc-sys` build script (about 70 s of a 3m16s clean build, mostly alone
  on the critical path), but gives up the post-quantum key exchange
  everywhere, and `reqwest` then panics on any client built before a provider
  is installed.
- Keep both. No post-quantum MoQT, two crypto libraries.

## Decision

Use `aws-lc-rs` everywhere and enable `prefer-post-quantum`. MoQT over QUIC
and WebSocket, and HTTPS from `reqwest`, offer and accept X25519MLKEM768; a
peer without it falls back to X25519. The `aws-lc-sys` build time stays.
The test in `crypto_provider.rs` fails if the feature is dropped.

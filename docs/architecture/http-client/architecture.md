# http-client

Builds the workspace's `reqwest` clients: `builder()` and `client()` replace
`reqwest::Client::builder` and `reqwest::Client::new`, and
`install_crypto_provider()` covers the libraries that build their own client
(`object_store` in `recorder`, the OTLP exporter in `telemetry`).
`clippy.toml` disallows the `reqwest` constructors everywhere else.

## Why a crate of its own

The workspace has one TLS crypto provider, ring: `moqt` installs it for QUIC
and WebSocket. `reqwest`'s `rustls` feature adds a second one, `aws-lc-rs`,
whose `aws-lc-sys` build script compiles AWS-LC with cmake for about 70 s of a
clean build, mostly alone on the critical path before `rustls`, `quinn` and
`moqt` can start. `object_store`'s `gcp` feature and `opentelemetry-otlp`'s
`reqwest-rustls` feature enable the same provider.

`reqwest`'s `rustls-no-provider` feature drops it, but then `reqwest` panics
when it builds a client before a process crypto provider is installed, and it
has no per-client way to choose ring while keeping its platform certificate
verifier. Installing ring in each binary's `main` leaves every test and every
library that builds a client one forgotten call away from that panic, so the
install sits next to the only constructors allowed to build a client.

`http-client` is also the one crate that turns on `reqwest`'s TLS feature;
the other crates take `reqwest` without one and get TLS through feature
unification.

## What ring gives up

`rustls` with `aws-lc-rs` offers the post-quantum hybrid key exchange
X25519MLKEM768; ring offers only X25519. HTTPS from these clients (VTS,
Cloud Storage, the Agent Platform, OTLP) is therefore not protected against a
recorded handshake being broken by a future quantum computer. MoQT over QUIC
already ran on ring, so no traffic it carries changes. None of these HTTPS
exchanges carry data that has to stay confidential for years.

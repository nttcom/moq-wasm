# reqwest for the IAP public keys

## Status

Accepted

## Date

2026-10-20

## What

Add `reqwest` (with the `rustls` and `json` features, no default features) as
a direct dependency of the `console` crate. `server.rs` takes `reqwest::Url`
for the IAP key URL it hands to `google_cloud::IapVerifier`, which fetches
the keys, and its tests call the server with a `reqwest::Client`.

## Context

The console verifies IAP assertions against keys that IAP rotates;
`google-cloud` fetches the key set over HTTPS and caches it.

## Alternatives

- `hyper` as an HTTP client. Already a dependency for the server, but TLS and
  connection handling would be hand-written.

## Decision

Use `reqwest`. It is already in the workspace lock with the same features
(`google-cloud`, `vts`), so no new crate enters the dependency tree.

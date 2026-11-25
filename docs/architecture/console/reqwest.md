# `reqwest` for IAP Public Keys (`console`)

**Status:** Accepted (2026-10-20)

## What

Add `reqwest` (`default-features = false`, features: `rustls`, `json`) as a direct dependency of `console`:

- `server.rs` passes `reqwest::Url` for the IAP key URL to `google_cloud::IapVerifier`.
- Server tests send requests using `reqwest::Client`.

## Context

`console` verifies Google Cloud IAP assertions against public keys that IAP rotates. `google-cloud` fetches and caches that key set over HTTPS.

## Alternatives

- **`hyper` as an HTTP client:** Already depended on for the server, but TLS and connection handling would be hand-written.

## Decision

Use `reqwest`. It is already in the workspace lock with the same features (`google-cloud`, `vts`), adding no new crate to the dependency tree.

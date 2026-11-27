# `reqwest` for IAP Public Keys (`console`)

**Status:** Accepted (2026-10-20)

## What

Add `reqwest` (`default-features = false`, features: `rustls`, `json`) as a direct dependency of `console`:

- `main.rs` / `identity.rs` pass `reqwest::Url` for the IAP key URL to `google_cloud::IapVerifier`.
- `usage.rs` (`ObservabilityClient`) and `recordings.rs` (`RecorderClient`) proxy console usage and recording requests with `reqwest::Client`.
- Server tests send requests using `reqwest::Client`.

## Context

`console` verifies Google Cloud IAP assertions against public keys that IAP rotates (`google-cloud` fetches and caches that key set over HTTPS) and queries `observability` and `recorder` over HTTP.

## Alternatives

- **`hyper` as an HTTP client:** Already depended on for the server, but TLS and connection handling would be hand-written.

## Decision

Use `reqwest`. It is already in the workspace lock with the same features (`google-cloud`, `bot-dispatcher`, `temporary-apps`), adding no new crate to the dependency tree.

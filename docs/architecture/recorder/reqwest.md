# `reqwest` for Launching Cloud Run Jobs (`recorder`)

**Status:** Accepted (2026-10-19)

## What

Add `reqwest` (`default-features = false`, features: `json`, `rustls`) as a direct dependency of `services/recorder`:

- `bot_job::composite_launcher` passes its client to `JobLauncher::CloudRun` (`job-launcher`), which invokes `POST https://run.googleapis.com/v2/<job>:run` with an access token from `google-cloud`'s `MetadataToken`.
- `recorder` also calls `bot-dispatcher` with it.

## Context

`recorder serve` delegates room recording and compositing to separate jobs so the API scales to zero on Cloud Run and each composite gets dedicated CPU. Starting Cloud Run Jobs via the Admin API requires an HTTPS client and a service-account OAuth token.

## Alternatives

- **`google-cloud-run` / `google-cloud-auth` SDK crates:** Typed requests and token caching, but adds a large generated client and separate error/runtime types for a single REST endpoint.
- **`hyper` + `hyper-rustls`:** Requires `hyper-util`'s legacy client, manual TLS setup, and manual JSON bodies.
- **Shelling out to `gcloud run jobs execute`:** Requires bundling the Cloud SDK and a shell in the image, and forces parsing CLI output for errors.

## Decision

Use `reqwest` with the same features as `job-launcher`. It is already in the workspace lock (via `object_store`'s `gcp` feature), adding no new crate. Tokens are fetched per launch rather than cached because launches are rare (one per recording or composite).

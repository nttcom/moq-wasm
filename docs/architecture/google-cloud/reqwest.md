# `reqwest` for the Cloud Storage JSON API (`google-cloud`)

**Status:** Accepted (2026-10-20)

## What

Add `reqwest` (`default-features = false`, features: `rustls`, `json`) as a direct dependency of `google-cloud`:

- `src/cloud_storage.rs`: downloads objects with their generation, and uploads or deletes objects conditionally with `ifGenerationMatch`.
- `src/metadata_token.rs`: fetches access tokens from the GCE metadata server.
- `src/jwt.rs`: fetches Google's JWKS.

## Context

In production, the app registry is a single Cloud Storage JSON object that VTS reads and `console` writes, and `recorder` releases a room lock only if no other request acquired it in the meantime. These workflows require four Cloud Storage requests and one metadata-server request. The decision originated in `app-registry` and moved to `google-cloud` when `recorder` needed the same operations.

## Alternatives

- **`google-cloud-storage` (official Rust client):** Covers the full Storage API and its own auth, but adds a large dependency tree for a handful of HTTP calls.
- **`object_store`:** Unified API over Cloud Storage, S3, and local files with conditional puts (used by `recorder` for media files), but lacks conditional delete.
- **`hyper` as an HTTP client:** Already in the workspace for servers, but TLS, connection pooling, and URL encoding would be hand-written.

## Decision

Use `reqwest` directly against the JSON API. It is already in the workspace lock (`observability`, `ingest-gateway pull`), adding no new crate to the dependency tree.

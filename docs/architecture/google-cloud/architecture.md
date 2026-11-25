# `google-cloud` Architecture

Minimal HTTP helpers (over `reqwest`) for Google Cloud APIs. Every call authenticates as the workload's own service account—no key file is ever read.

## Modules

| Module | Key types | Responsibility |
| --- | --- | --- |
| `metadata_token.rs` | `MetadataToken`, `identity_token` | Fetches service-account OAuth access tokens (`Authorization: Bearer`) and audience-scoped Google ID tokens from the metadata server. |
| `cloud_storage.rs` | `CloudStorageBucket`, `Endpoint` | Downloads objects with generation and uploads with `ifGenerationMatch` via the Cloud Storage JSON API. |
| `jwt.rs` | `IapVerifier`, `IdTokenVerifier` | Verifies ES256 IAP assertions and RS256 Google ID tokens using a shared JWKS cache. |
| `testing.rs` | `FakeCloudStorage`, `SilentServer`, `TestIapKey` | In-memory test doubles on `http-server`'s `HttpServer` (behind the `testing` feature). |

## Key Details

- **Metadata tokens (`metadata_token.rs`)**:
  - `MetadataToken` reads `instance/service-accounts/default/token` on Cloud Run / Compute Engine and caches it until 1 minute before expiry. Outside Google Cloud, the call fails, so services that also run locally pair it with an environment token fallback.
  - `identity_token` reads `instance/service-accounts/default/identity` for a target audience (used, e.g., when a recorder job authenticates to `bot-dispatcher`).
- **Storage endpoints (`cloud_storage.rs`)**: `Endpoint` selects Cloud Storage (attaches `MetadataToken`) or a local emulator via `STORAGE_EMULATOR_HOST` (no token).
- **JWKS cache (`jwt.rs`)**: 5-second request timeout and 60-second refresh throttle, shared by both verifiers.

## Callers

| Caller | Uses |
| --- | --- |
| `app-registry` | `CloudStorageBucket` for the app registry object |
| `console` | `IapVerifier` for IAP assertion verification |
| `recorder` | `identity_token` for calling `bot-dispatcher` |
| `job-launcher` | `MetadataToken` to start Cloud Run Jobs |
| `bot-dispatcher` | `MetadataToken` (Cloud Run Jobs) and `IdTokenVerifier` (bot ID tokens) |
| `live-translate` | `MetadataToken` for Agent Platform calls |

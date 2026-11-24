# google-cloud

Minimal Google Cloud HTTP client using workload identity (no service-account key files): metadata server tokens, conditional Cloud Storage reads/writes, and Google IAP / ID token JWKS verification.

> **Workspace crate**: Internal library shared by [`app-registry`](../app-registry/README.md), [`job-launcher`](../job-launcher/README.md), `console`, `bot-dispatcher`, `live-translate`, and `recorder`. Native-only.

## Usage

Both examples run inside an `async fn` returning `anyhow::Result<()>`.

### Conditional Cloud Storage read/write

```rust
use std::time::Duration;
use google_cloud::{CloudStorageBucket, Endpoint, Uploaded};

let bucket = CloudStorageBucket::new(
    Endpoint::from_env()?, // uses STORAGE_EMULATOR_HOST if set, otherwise GCS
    "my-bucket",
    Duration::from_secs(2),
)?;

let current = bucket.download("registry.json").await?;
match bucket
    .upload_if_generation_matches(
        "registry.json",
        "application/json",
        new_bytes, // Vec<u8>
        current.generation,
    )
    .await?
{
    Uploaded::Written => {}
    Uploaded::Conflict => { /* HTTP 412: reload and retry */ }
}
```

### Verify an IAP assertion or Google ID token

```rust
use google_cloud::{GOOGLE_IAP_KEYS_URL, IapVerifier};

let verifier = IapVerifier::new(audience, GOOGLE_IAP_KEYS_URL.parse()?)?;
let caller = verifier.verify(iap_jwt_header, now_unix_secs).await?;
```

Use `IdTokenVerifier::new(GOOGLE_CERTS_URL.parse()?, audience, service_accounts)?` to verify RS256 Google OIDC ID tokens from allowed service accounts.

## Key types & features

| Item | Role |
| --- | --- |
| `MetadataToken`, `identity_token` | Fetches OAuth access tokens (cached) and OIDC ID tokens (uncached) from `http://metadata.google.internal` |
| `CloudStorageBucket`, `Endpoint`, `Versioned`, `Uploaded` | Conditional GCS JSON API `download()` and `upload_if_generation_matches()` |
| `IapVerifier`, `IapCaller`, `GOOGLE_IAP_KEYS_URL` | Verifies ES256 Cloud IAP JWT assertions against Google's JWK set |
| `IdTokenVerifier`, `GOOGLE_CERTS_URL` | Verifies RS256 Google ID tokens for an allowlist of service account emails |
| `testing` feature | Exposes `testing::{FakeCloudStorage, SilentServer, TestIapKey}` for tests (`[dev-dependencies]` only) |

## Behavior & caveats

- **Workload identity only**: `MetadataToken` and `identity_token` call `metadata.google.internal` with a 5 s timeout and fail outside GCP. `MetadataToken` caches tokens until `expires_in - 60 s`.
- **Emulator support**: Set `STORAGE_EMULATOR_HOST` for local testing (`Endpoint::from_env()` switches to `Endpoint::Emulator`, omitting auth headers).
- **GCS precondition**: Passing `generation = 0` to `upload_if_generation_matches` creates the object only if it does not exist; HTTP `412` maps to `Ok(Uploaded::Conflict)`.
- **JWKS refresh & clock skew**: Unknown `kid` triggers a JWKS refetch at most once per 60 s after a successful fetch (failed fetches do not start the cooldown, so subsequent calls retry immediately). `IapVerifier` allows ±30 s clock skew on `iat`/`exp`; `IdTokenVerifier` checks strict `exp > now` and `email_verified: true`.

## Further reading

- Architecture: [docs/architecture/google-cloud/architecture.md](../../docs/architecture/google-cloud/architecture.md)

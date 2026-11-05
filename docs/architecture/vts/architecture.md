# `vts` Architecture

## Status
Living document. Update this file in the same change whenever the design intent,
module boundaries, runtime flow, or invariants described here change.

## Scope
The `vts` crate is the Verify Token Service: an HTTP service the relay calls
once per session to check that a client's JWT was signed with the secret of a
registered app, and whether that app is a relay. Signing tokens is
`auth-token`'s CLI, not the VTS's.

Besides the registered apps, the VTS knows temporary apps: an `appId` with
`auth_token::TEMPORARY_APP_ID_PREFIX` (`temp-`) exists only as the tokens
issued for it, all signed with one secret (`VTS_TEMPORARY_APP_SECRET`). They
let anyone get an app of their own without registering one; the service that
issues them holds the same secret.

The service deliberately answers only the signature question. Token lifetime
(`iat`, `exp`, the maximum TTL, clock leeway) and the meaning of `publish` /
`subscribe` are the relay's policy (`services/relay/src/modules/auth/`), so one
clock and one rule set govern both accepting a session and expiring it later.

## Modules

| Module | Responsibility |
| --- | --- |
| `registry.rs` | `Registry`: reads the apps through `app-registry` on every `current` call and keeps the last apps it read. A read that fails logs a warning and returns those last apps. `Registry::load` fails when the first read fails. |
| `verify.rs` | `verify_token`: `auth_token::decode_unverified_payload` → require string `appId` → pick the secret: the temporary app secret for a temporary app when one is configured, else the registered app's → `auth_token::verify_signature` with it. Returns `VerifiedToken { app_id, is_relay, claims }` or a `RejectReason`; a `SignatureError` maps onto `malformed_token` or `invalid_signature`. |
| `server.rs` | `VtsServer`: owns the accept-loop `JoinHandle`; one `tokio::spawn` per connection running hyper HTTP/1. Routes `GET /healthz` and `POST /verify` against `Secrets` (the `Registry` and the optional temporary app secret); everything else is 404. |
| `main.rs` | Picks the registry location (`VTS_APPS_FILE` or `VTS_REGISTRY_BUCKET`, not both), loads the `Registry`, reads `VTS_TEMPORARY_APP_SECRET`, binds `0.0.0.0:<port>` and serves until the process is terminated. A registry that cannot be loaded at startup stops the process. |

The registered apps are `app_registry::Apps`; parsing, validation and storage live in `crates/app-registry`.

## Request flow

```mermaid
sequenceDiagram
    participant R as relay
    participant S as server.rs
    participant V as verify.rs
    R->>S: POST /verify {token}
    S->>S: read body (≤ 16 KiB) and parse JSON
    alt body invalid
        S-->>R: 400 {error: invalid_request}
    end
    S->>S: Registry::current (read the registry, or the last apps read)
    S->>V: verify_token(token, apps)
    V->>V: decode_unverified_payload → appId
    alt temporary app and secret configured
        V->>V: verify_signature(token, temporary secret)
    else
        V->>V: apps.get(appId)
        V->>V: verify_signature(token, app.secret)
    end
    alt rejected
        S-->>R: 401 {error: malformed_token | unknown_app | invalid_signature}
    else verified
        S-->>R: 200 {appId, isRelay, claims}
    end
```

## Invariants

- `claims` in the 200 response is the signed payload as-is. The VTS never
  adds, removes or interprets claims; the relay's `SignedToken` reads them.
- Rejections are reported in the order they are detected: `malformed_token`
  (not three segments, payload not a JSON object, or no non-empty string
  `appId`), then `unknown_app`, then `invalid_signature`. A header whose
  `alg` is anything other than `HS256` is `invalid_signature`, matching the
  previous Node.js implementation built on `jose`, so the relay's
  `AuthenticationError` mapping is unchanged.
- Signing secrets exist only in the registry and, for temporary apps, in
  `VTS_TEMPORARY_APP_SECRET`. The relay never receives them.
- A temporary app is never a relay. While the temporary secret is configured,
  every `temp-` app is verified with it, so a registered app with that prefix
  is shadowed; without it, a `temp-` app is looked up in the registry and is
  normally `unknown_app`.
- Every `/verify` with a valid body reads the registry, so a rotated secret or
  a deleted app is refused from the next verification on. A request with an
  invalid body is answered before the registry is read.
- While the registry cannot be read, `/verify` keeps verifying with the apps
  last read, so revocations made during that time take effect only once it can
  be read again. A new instance that cannot read the registry does not start.
- The apps last read are replaced by whichever read finishes last; two reads
  that overlap may leave the one that started earlier. Those apps are used
  only while the registry cannot be read.

## Operational notes

- Logging goes through `tracing`, set up by `crates/telemetry`: `verify ok` /
  `verify rejected` at INFO, per-connection HTTP errors at DEBUG.
  `VTS_STDOUT_FILTER`, `VTS_OTEL_FILTER` and `VTS_LOG_FILTER` filter, each
  falling back to `RUST_LOG`.
- Every request runs in a `vts.request` span that continues the caller's
  trace from its `x-original-traceparent` header, else its W3C `traceparent`
  (see "Trace context" in the telemetry architecture), so a verification
  shows up under the relay's `relay.vts.verify` span. A `/verify` that
  reaches the registry records `outcome` on the span: `verified`, with the
  token's `app_id` and `is_relay`, or the rejection reason that
  `verify rejected` logs. The token and secrets stay out of the span.
- The Docker image (`services/vts/Dockerfile`) is a `cargo-chef` build onto
  `distroless/cc`, the same base as the relay, and ships `vts`.

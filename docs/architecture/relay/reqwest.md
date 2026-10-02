# reqwest for calling the token verification service

## Status

Accepted

## Date

2026-09-09

## What

Add `reqwest` (with the `json` and `rustls` features, no default features) as
a direct dependency of the `relay` crate. It is used by
`modules/auth/vts_token_verifier.rs` to `POST` the client's authorization token
to the Verify Token Service (VTS) and read back the verified claims.

## Context

Clients present a JWT in the CLIENT_SETUP AUTHORIZATION TOKEN parameter. The
relay does not hold signing secrets; it forwards the token to VTS over HTTP
once per session and trusts the returned claims. This is the relay's first
outbound HTTP call.

## Alternatives

- Verify the JWT locally with `jsonwebtoken`. Requires distributing every
  appId's secret to every relay and rotating them in lockstep; rejected in the
  auth design in favour of a single verification point.
- `hyper` directly. Lower level than needed for one JSON round trip; TLS,
  connection pooling and timeouts would be hand-written.
- `ureq` (blocking). Would need `spawn_blocking` inside the async accept path.

## Decision

Use `reqwest`. It is already in the workspace lock at 0.13.4 (a direct
dependency of `bridges/onvif` and a transitive one of `opentelemetry-otlp`
inside `relay`), so no new crate enters the dependency tree. The `rustls`
feature matches the TLS stack the relay already links for QUIC.

# `reqwest` for Calling the Verify Token Service

## Status
Accepted (2026-09-09)

## What
Optional dependency of `auth-token` (`json`, `rustls`, `default-features = false`, gated behind `vts-client`). `VtsClient` (`vts_client.rs`) POSTs client JWTs to VTS and parses verified claims for `relay`, `bot-dispatcher`, `sip-gateway`, `observability`, and `recorder`.

## Context
Relays and services do not hold app signing secrets; they verify client JWTs via a single HTTP call to VTS per session.

## Alternatives
- **Local `jsonwebtoken` verification**: Requires distributing and rotating every app's secret across all relays.
- **`hyper` directly**: Requires hand-writing TLS, pooling, and timeouts for one JSON POST.
- **`ureq` (blocking)**: Requires `spawn_blocking` on the async accept path.

## Decision
Use `reqwest` (`rustls` matches the relay's QUIC TLS stack) behind `vts-client`, so sign-only/check-only crates do not link an HTTP client.

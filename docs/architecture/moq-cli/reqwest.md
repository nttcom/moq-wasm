# `reqwest` for Exporting CLI Logs (`moq-cli`)

**Status:** Accepted (2026-11-02)

## What

Add `reqwest` (`default-features = false`, features: `json`, `rustls`) as a direct dependency of `moq-cli`. `telemetry.rs` `POST`s CLI logs to the observability server's `/api/client-telemetry` endpoint every 10 s and once on exit.

## Context

Previously, CLI logs went only to `stderr`, leaving no client-side traces in HyperDX. Because the OTLP collector listens only inside the VPC while `moq-cli` typically runs on a developer laptop, it must post to the observability server's public HTTP endpoint, just like browser clients.

## Alternatives

- **`crates/telemetry-otlp`:** Only works where the OTLP collector is directly reachable (Docker Compose or inside the VPC), not from laptops targeting deployed relays.
- **`hyper` directly:** Requires hand-writing TLS, timeouts, and connection reuse for a single `POST`.
- **`ureq` (blocking):** Requires `spawn_blocking` inside Tokio and adds a new crate to the workspace.

## Decision

Use `reqwest`. It is already in the workspace lock (`observability`, `recorder`, and others), adding no new crate to the dependency tree.

# reqwest for sending the CLI's logs to the observability server

## Status

Accepted

## Date

2026-11-02

## What

Add `reqwest` (with the `rustls` feature, no default features) as a direct
dependency of `moq-cli`. `telemetry/report_task.rs` uses it to `POST` the
CLI's logs to the observability server's `/api/client-telemetry` every 10 s
and once more on exit.

## Context

The CLI's logs only reached its stderr, so a session from the CLI left no
client-side trace in HyperDX. The collector listens only inside the VPC, and
the CLI usually runs on a laptop, so it cannot export OTLP itself: it posts to
the observability server's public endpoint, as the browser pages do.

## Alternatives

- `crates/telemetry` (OTLP export to the collector). Works only where the
  collector is reachable (docker compose, inside the VPC), not from a laptop
  to the deployed relays.
- `hyper` directly. TLS, timeouts and connection reuse would be hand-written
  for one POST.
- `ureq` (blocking). Would need `spawn_blocking` inside the tokio runtime, and
  is a crate the workspace does not have yet.

## Decision

Use `reqwest`. It is already in the workspace lock at 0.13 (a direct
dependency of `relay`, `observability` and others), so no new crate enters
the dependency tree.

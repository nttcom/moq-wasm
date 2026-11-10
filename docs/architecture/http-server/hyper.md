# hyper, hyper-util and http-body-util for the shared HTTP server

## Status

Accepted

## Date

2026-11-10

## What

Add `hyper` 1.x (features `http1`, `server`), `hyper-util` (feature `tokio`)
and `http-body-util` as direct dependencies of the `http-server` crate. It
runs the HTTP/1 accept loop and builds the `Full<Bytes>` JSON bodies the
services share.

## Context

`console`, `vts`, `token-mint`, `observability`, `live-translate` and
`recorder` each copied the same accept loop and JSON response helpers on these
three crates. Moving them into one crate moves the dependencies with them.

## Alternatives

- Keep the copies in each service. Every change to the accept loop or its logs
  has to be made six times.
- `axum`. Already rejected for the VTS (see `docs/architecture/vts/hyper.md`):
  it adds a framework for routing the services do with a `match`.

## Decision

Use the crates the six services already depend on. They are already compiled
in the workspace, so no new crate enters the dependency tree.

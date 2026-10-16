# hyper for the recording API

## Status

Accepted

## Date

2026-10-16

## What

Add `hyper` (`http1`, `server`), `hyper-util` (`tokio`) and `http-body-util`
to `crates/recorder`. `src/api_server.rs` serves the API of
`recorder serve`, which starts, stops and reports room recordings for the
meeting example.

## Context

A browser page has to start and stop a recording, so the recorder needs an
HTTP server: four JSON endpoints, a small request body, and the CORS answers a
browser asks for. `observability` already serves its API with the same three
crates (see `docs/architecture/observability/hyper.md`), so they are in the
workspace's lock file.

## Alternatives

- `axum`. Routing and extractors would shorten the handlers, but it brings
  `axum` and `tower` for four routes the match in `handle` covers.
- Starting `recorder` processes from a separate service. The recorder would
  stay a CLI, but the service would duplicate the room tracking and parse the
  recorder's logs to report progress.

## Decision

Use `hyper` directly, the same way `observability` does. JSON is read and
written with `serde_json`, which the recorder already depends on, so no
`serde` derive is added.

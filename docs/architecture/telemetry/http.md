# http for the trace context helpers

## Status

Accepted

## Date

2026-10-27

## What

`telemetry` depends on `http` 1.x so `inject_trace_context` and
`continue_trace_from` read and write W3C `traceparent` headers in an
`http::HeaderMap`.

## Context

The relay calls the VTS with `reqwest` and the VTS serves with `hyper`. Both
expose their headers as `http::HeaderMap`, so one pair of helpers taking that
type covers the client and the server.

## Alternatives

- `opentelemetry-http`, which ships the same `HeaderMap` injector and
  extractor. It also pulls in an HTTP client abstraction the helpers do not
  use.
- Helpers taking a `HashMap<String, String>`. No dependency, but each caller
  converts to and from its `HeaderMap`.

## Decision

Depend on `http` directly and implement the two small `Injector` /
`Extractor` adapters in `telemetry`. `http` is already compiled in the
workspace through `reqwest` and `hyper`, so no new crate enters `Cargo.lock`.

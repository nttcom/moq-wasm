# `http` for Trace-Context Header Helpers (`telemetry-otlp`)

**Status:** Accepted (2026-10-27)

## What

Add `http` 1.x to `telemetry-otlp` so `inject_trace_context` and `continue_trace_from` read and write W3C `traceparent` headers directly on `http::HeaderMap`.

## Context

`relay` calls VTS with `reqwest` and VTS serves requests with `hyper`. Both represent HTTP headers as `http::HeaderMap`, allowing one pair of helpers to serve both client and server.

## Alternatives

- **`opentelemetry-http`:** Provides `HeaderMap` injector/extractor helpers, but pulls in an unused HTTP client abstraction.
- **`HashMap<String, String>` helpers:** Avoids the dependency, but forces every caller to convert to and from `HeaderMap`.

## Decision

Depend directly on `http` and implement the two small `Injector`/`Extractor` adapters in `telemetry-otlp`. `http` is already in `Cargo.lock` via `reqwest` and `hyper`, adding no new crate.

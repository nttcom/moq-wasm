# hyper for the console HTTP server

## Status

Accepted

## Date

2026-10-20

## What

Add `hyper` 1.x (features `http1`, `server`) as a direct dependency of the
`console` crate. `server.rs` serves the console API over HTTP/1 with it.

## Context

The console answers a handful of JSON routes behind IAP on Cloud Run. The VTS
already serves its routes with `hyper` and the same `service_fn` and `match`
on method and path ([vts/hyper.md](../vts/hyper.md)).

## Alternatives

- `axum`. Shorter route declarations and extractors, but adds `axum`,
  `axum-core`, `tower`, `tower-layer`, `tower-service` and `matchit` to the
  dependency tree for a few routes with no middleware.
- Hand-written HTTP/1 on `tokio::net::TcpStream`. Would reimplement request
  parsing, keep-alive and framing.

## Decision

Use `hyper` directly, as the VTS does. It is already in the workspace lock, so
no new crate enters the dependency tree.

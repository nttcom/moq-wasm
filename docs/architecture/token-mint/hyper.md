# hyper for the token mint HTTP server

## Status

Accepted

## Date

2026-11-05

## What

Add `hyper` 1.x (features `http1`, `server`) as a direct dependency of the
`token-mint` crate. `server.rs` serves `POST /tokens`,
`POST /tokens/refresh` and `GET /healthz` over HTTP/1 with it.

## Context

The token mint needs an HTTP/1 server for three routes, a CORS preflight, a
16 KiB body limit and JSON bodies: the same shape as the VTS server.

## Alternatives

- `axum`. Shorter route declarations, but adds `axum`, `axum-core`, `tower`,
  `tower-layer`, `tower-service` and `matchit` to the dependency tree for three
  routes with no middleware.
- Hand-written HTTP/1 on `tokio::net::TcpStream`. Would reimplement request
  parsing, keep-alive and framing that `hyper` already gets right.

## Decision

Use `hyper` directly, as the VTS and the console do. It is already compiled in
the workspace, so no new crate enters the dependency tree, and `service_fn`
plus a `match` on method and path is enough routing.

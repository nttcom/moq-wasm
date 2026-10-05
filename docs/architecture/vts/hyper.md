# hyper for the VTS HTTP server

## Status

Accepted

## Date

2026-10-05

## What

Add `hyper` 1.x (features `http1`, `server`) as a direct dependency of the
`vts` crate. `server.rs` serves `GET /healthz` and `POST /verify` over HTTP/1
with it.

## Context

The VTS was a Node.js service on `node:http`. Rewriting it in Rust needs an
HTTP/1 server for two routes, a 16 KiB body limit and JSON bodies. The relay
already calls the VTS with `reqwest`, which is built on `hyper`.

## Alternatives

- `axum`. Shorter route declarations, but adds `axum`, `axum-core`, `tower`,
  `tower-layer`, `tower-service` and `matchit` to the dependency tree for two
  routes with no middleware.
- `actix-web`, `warp`, `rocket`. Full frameworks with their own runtimes or
  filter DSLs; far more than two routes need.
- Hand-written HTTP/1 on `tokio::net::TcpStream`. Would reimplement request
  parsing, keep-alive and framing that `hyper` already gets right.

## Decision

Use `hyper` directly. It is already compiled in the workspace (a transitive
dependency of `reqwest` and `tonic`), so no new crate enters the dependency
tree, and `service_fn` plus a `match` on method and path is enough for the
VTS's routing.

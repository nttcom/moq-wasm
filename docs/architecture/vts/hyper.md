# `hyper` for the VTS HTTP Server (`vts`)

**Status:** Accepted (2026-10-05)

## What

Add `hyper` 1.x (`http1`, `server`) as a direct dependency of `vts`. `server.rs` serves `GET /healthz` and `POST /verify` over HTTP/1.

## Context

Rewriting VTS from Node.js (`node:http`) to Rust requires an HTTP/1 server for two JSON routes with a 16 KiB body limit. `relay` already calls VTS via `reqwest`, which depends on `hyper`.

## Alternatives

- **`axum`:** Pulls in `axum`, `axum-core`, `tower`, `tower-layer`, `tower-service`, and `matchit` for two routes with no middleware.
- **`actix-web`, `warp`, `rocket`:** Full frameworks with custom runtimes or filter DSLs—excessive for two endpoints.
- **Hand-written HTTP/1 on `tokio::net::TcpStream`:** Reimplements request parsing, keep-alive, and framing.

## Decision

Use `hyper` directly with `http-server`'s accept loop and a `match` on method and path. `hyper` is already in the workspace (transitively via `reqwest` and `tonic`), adding no new crate to the dependency tree.

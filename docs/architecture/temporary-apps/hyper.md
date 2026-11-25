# `hyper` for the HTTP Server (`temporary-apps`)

**Status:** Accepted (2026-11-05)

## What

Add `hyper` 1.x (`http1`, `server`) as a direct dependency of `temporary-apps`. `server.rs` serves `POST /tokens`, `POST /tokens/refresh`, `/bots` routes, and `GET /healthz` over HTTP/1 using `http-server`'s accept loop.

## Context

`temporary-apps` needs an HTTP/1 server with a small flat route set, CORS preflight handling, a 16 KiB body cap, and JSON payloads—the same profile as VTS.

## Alternatives

- **`axum`:** Shorter route syntax, but pulls in `axum`, `axum-core`, `tower`, `tower-layer`, `tower-service`, and `matchit` for a handful of routes with no middleware.
- **Hand-written HTTP/1 on `tokio::net::TcpStream`:** Reimplements request parsing, keep-alive, and framing.

## Decision

Use `hyper` directly with a method/path `match`, matching VTS and `console`. It is already compiled in the workspace, adding no new crate to the dependency tree.

# `hyper` for the `console` HTTP Server

## Status
Accepted (2026-10-20)

## What
Direct dependency of `console` (`hyper` 1.x: `http1`, `server`). `server.rs` serves the console HTTP/1 API.

## Context
`console` exposes a handful of JSON routes behind IAP on Cloud Run, using the same `service_fn` + method/path `match` pattern as VTS ([`vts/hyper.md`](../vts/hyper.md)).

## Alternatives
- **`axum`**: Adds `axum`, `axum-core`, `tower*`, and `matchit` for a few middleware-free routes.
- **Raw HTTP/1 over `TcpStream`**: Reimplements parsing, keep-alive, and framing.

## Decision
Use `hyper` directly (already in the workspace lock, matching VTS).

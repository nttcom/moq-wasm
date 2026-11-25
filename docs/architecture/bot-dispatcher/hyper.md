# `hyper` for the `bot-dispatcher` HTTP Server

## Status
Accepted (2026-11-18)

## What
Direct dependency of `bot-dispatcher` (`hyper` 1.x: `http1`, `server`). `server.rs` accepts `hyper::Request<Incoming>` from `http_server::HttpServer` and returns `http_server::JsonResponse`.

## Context
`bot-dispatcher` serves six JSON routes with no middleware, matching `vts` and `temporary-apps`.

## Alternatives
- **`axum`**: Pulls in `tower` and router crates for six plain routes.

## Decision
Use `hyper` via `http-server`, matching workspace services with no new dependency.

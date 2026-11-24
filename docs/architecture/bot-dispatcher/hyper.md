# hyper for the bot-dispatcher HTTP server

## Status

Accepted

## Date

2026-11-18

## What

Add `hyper` 1.x (`http1`, `server`) as a direct dependency of
`bot-dispatcher`. `server.rs` takes `hyper::Request<Incoming>` from
`http_server::HttpServer` and answers `http_server::JsonResponse`.

## Context

`bot-dispatcher` serves six routes with JSON bodies, as the VTS and `temporary-apps` do.

## Alternatives

- `axum`. Shorter routing, but adds `tower` and its crates for six routes
  with no middleware.

## Decision

Use `hyper` through `http-server`, as the other services do. It is already
compiled in the workspace.

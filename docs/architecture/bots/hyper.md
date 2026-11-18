# hyper and http-body-util for the bots HTTP server

## Status

Accepted

## Date

2026-11-18

## What

Add `hyper` 1.x (`http1`, `server`) and `http-body-util` as direct
dependencies of `bots`. `server.rs` takes `hyper::Request<Incoming>` from
`http_server::HttpServer` and answers `Response<Full<Bytes>>`.

## Context

`bots` serves six routes with JSON bodies, as the VTS and the token mint do.

## Alternatives

- `axum`. Shorter routing, but adds `tower` and its crates for six routes
  with no middleware.

## Decision

Use `hyper` through `http-server`, as the other services do. Both crates are
already compiled in the workspace.

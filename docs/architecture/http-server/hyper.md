# `hyper`, `hyper-util`, and `http-body-util` for the Shared HTTP Server (`http-server`)

**Status:** Accepted (2026-11-10)

## What

Add `hyper` 1.x (`http1`, `server`), `hyper-util` (`tokio`), and `http-body-util` as direct dependencies of `http-server` to provide the shared HTTP/1 accept loop and `Full<Bytes>` JSON response helpers.

## Context

`console`, `vts`, `temporary-apps`, `observability`, `live-translate`, and `recorder` each duplicated the same accept loop and JSON helpers on these three crates. Extracting them into `http-server` consolidates both code and dependencies.

## Alternatives

- **Keep copies in each service:** Every change to the accept loop or its logging must be repeated six times.
- **`axum`:** Previously rejected for VTS (see [vts/hyper.md](../vts/hyper.md)); adds a routing framework where services use a plain `match`.

## Decision

Use `hyper`, `hyper-util`, and `http-body-util`. All six services already compile them, adding no new crate to the dependency tree.

# `url` for Shared Query-String Parsing (`http-server`)

**Status:** Accepted (2026-11-19)

## What

Add `url` 2.x as a direct dependency of `http-server`. `query_params` uses `url::form_urlencoded::parse` to decode request URI queries.

## Context

`recorder`, `observability`, and `console` each parsed queries by wrapping them in a synthetic `http://query/?…` URL and reading `query_pairs`, while `bot-dispatcher` called `url::form_urlencoded` directly. One shared parser in `http-server` replaces all four.

## Alternatives

- **`form_urlencoded` directly:** Re-exported by `url`, but no workspace crate depends on it directly, adding another dependency to track.
- **Keep the synthetic-URL helper in each service:** Duplicates parsing logic across services.

## Decision

Use `url`. `bot-dispatcher` and `recorder` already depend on it and `reqwest` already compiles it, adding no new crate to the dependency tree.

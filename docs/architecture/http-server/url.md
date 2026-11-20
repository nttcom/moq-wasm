# url for the shared query string parser

## Status

Accepted

## Date

2026-11-19

## What

Add `url` 2.x as a direct dependency of the `http-server` crate. Its
`form_urlencoded::parse` decodes a request URI's query in `query_params`.

## Context

`recorder`, `observability` and `console` each parsed a query by wrapping it in
a fake `http://query/?…` URL and reading its `query_pairs`, and `bot-dispatcher` called
`url::form_urlencoded` itself. One parser in `http-server` replaces the four.

## Alternatives

- `form_urlencoded` directly. It is the crate `url` re-exports, but no crate in
  the workspace names it, so it would be one more direct dependency to track.
- Keep the fake-URL trick in each service. Every service repeats it.

## Decision

Use `url`, which `bot-dispatcher` and `recorder` already depend on and `reqwest` already
compiles, so no new crate enters the dependency tree.

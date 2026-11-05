# http-body-util for bounded body collection

## Status

Accepted

## Date

2026-11-05

## What

Add `http-body-util` as a direct dependency of the `token-mint` crate for
`Limited` (reject refresh bodies over 16 KiB), `BodyExt::collect` (read the
body into bytes) and `Full` (the fixed JSON response body).

## Context

`hyper` 1.x exposes bodies only through the `http_body::Body` trait and ships
no combinators of its own. The refresh endpoint must cap the body size before
buffering it, and every route answers with a complete JSON body.

## Alternatives

- Drive `Body::poll_frame` by hand and count bytes. Reimplements `Limited`
  and `collect`.

## Decision

Use `http-body-util`, the body companion crate maintained alongside `hyper`
and already in the workspace lock.

# http-body-util for bounded body collection

## Status

Accepted

## Date

2026-10-05

## What

Add `http-body-util` as a direct dependency of the `vts` crate for
`Limited` (reject request bodies over 16 KiB), `BodyExt::collect` (read the
body into bytes) and `Full` (the fixed JSON response body).

## Context

`hyper` 1.x exposes bodies only through the `http_body::Body` trait and ships
no combinators of its own. The VTS needs to cap the request body size before
buffering it and to answer with a complete JSON body.

## Alternatives

- Drive `Body::poll_frame` by hand and count bytes. Reimplements `Limited`
  and `collect`.
- Read the `Content-Length` header only. Misses chunked bodies and still
  requires a body type for responses.

## Decision

Use `http-body-util`. It is the body companion crate maintained alongside
`hyper` and is already in the workspace lock via `reqwest`.

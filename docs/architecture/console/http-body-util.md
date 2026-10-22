# http-body-util for response bodies

## Status

Accepted

## Date

2026-10-20

## What

Add `http-body-util` as a direct dependency of the `console` crate for `Full`,
the complete JSON body of every console response.

## Context

`hyper` 1.x exposes bodies only through the `http_body::Body` trait and ships
no body types of its own. The console answers every request with a complete
JSON body.

## Alternatives

- Implement `http_body::Body` for a local bytes wrapper. Reimplements `Full`.

## Decision

Use `http-body-util`, the body companion crate maintained alongside `hyper`.
It is already in the workspace lock, so nothing new is compiled.

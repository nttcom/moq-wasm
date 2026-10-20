# thiserror for SignatureError

## Status

Accepted

## Date

2026-10-20

## What

Add `thiserror` as a direct dependency of the `auth-token` crate to derive
`std::error::Error` and `Display` for `SignatureError`.

## Context

`verify_signature` reports two kinds of failure, a malformed token and a
signature that does not match. The VTS branches on the kind to answer
`malformed_token` or `invalid_signature`, so the failure is a typed error
rather than an `anyhow::Error`. The coding style asks for a `thiserror` enum
in that case.

## Alternatives

- Implement `Display` and `std::error::Error` by hand. Same result with more
  code to keep in step with the variants.
- Return a plain enum that is not an `Error`. Callers could not use `?` into
  `anyhow::Result`.

## Decision

Use `thiserror`. It is already in the workspace lock, so it adds no new
compile unit, and the derive keeps each message next to its variant.

# ring for HMAC-SHA256

## Status

Accepted

## Date

2026-10-05

## What

Add `ring` as a direct dependency of the `vts` crate. `jwt.rs` signs and
verifies HS256 JWTs with `ring::hmac` (`HMAC_SHA256`, constant-time
`verify`).

## Context

VTS tokens are HS256 JWTs. The Node.js implementation used `jose`; the Rust
crate needs one HMAC-SHA256 primitive plus base64url, and the relay decides
everything else about the claims.

## Alternatives

- `jsonwebtoken`. Full JWT library whose `Validation` checks `exp` by default
  and requires an explicit opt-out; the VTS must pass time claims through
  untouched. New crate in the tree.
- `hmac` + `sha2` (RustCrypto). Only `sha2` 0.9 and `hmac` 0.10 / 0.12 are in
  the lock through other paths; the current `sha2` 0.10 would be a new
  compile unit, and two crates instead of one.
- `josekit`, `jwt-simple`. Larger surface and new dependencies for one
  algorithm.

## Decision

Use `ring`. It is already compiled in the workspace as the crypto provider
under `rustls` (QUIC and the relay's TLS), its HMAC API is a dozen lines for
sign and verify, and `verify` is constant-time. Header and payload encoding
are done in `jwt.rs` with the `base64` crate, which other workspace crates
already depend on.

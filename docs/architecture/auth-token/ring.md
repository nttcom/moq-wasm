# ring for HMAC-SHA256

## Status

Accepted

## Date

2026-10-20

## What

Add `ring` as a direct dependency of the `auth-token` crate. `jwt.rs` signs and
verifies HS256 JWTs with `ring::hmac` (`HMAC_SHA256`, constant-time
`verify`).

## Context

Client tokens are HS256 JWTs. The VTS verifies them, and both the `auth-token` CLI and
the console sign them, so signing and verification live next to the claims
they carry instead of inside the VTS. The crate needs one HMAC-SHA256
primitive plus base64url; the relay decides everything else about the claims.

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
under `rustls` and `quinn`, so the relay, which depends on `auth-token` only
for `Claims`, gains no new crate. Its HMAC API is a dozen lines for sign and
verify, and `verify` is constant-time. Header and payload encoding use the
`base64` crate, which `auth-token` already depends on.

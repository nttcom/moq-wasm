# aws-lc-rs for ID token signatures, bot ids and bot keys

## Status

Accepted

## Date

2026-11-18

## What

Add `aws-lc-rs` as a direct dependency of `bot-dispatcher`. `id_token.rs` verifies the
RS256 signature of a Google ID token with `RsaPublicKeyComponents`, and
`bots.rs` draws bot ids and local bot keys from `SystemRandom`, hashes a key
with SHA-256 and compares it in constant time.

## Context

A bot authenticates to `bot-dispatcher` with the ID token of its job's service account
or, launched locally, with a random key. Verifying the first needs RSA; the
second needs random bytes and a hash.

## Alternatives

- `jsonwebtoken`. Verifies the JWT in one call, but brings its own crypto
  backend and JWK types for a check of four claims.
- `rsa` plus `sha2` and `rand`. Three new crates for what one already in the
  tree does.

## Decision

Use `aws-lc-rs`: `auth-token` already compiles it for HS256, and it covers
RSA verification, randomness, SHA-256 and constant-time comparison.

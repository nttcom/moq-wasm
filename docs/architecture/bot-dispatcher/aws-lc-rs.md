# aws-lc-rs for ID token signatures, bot ids and bot keys

## Status

Accepted

## Date

2026-11-18

## What

Add `aws-lc-rs` as a direct dependency of `bot-dispatcher`. `bots.rs` draws
bot ids and local bot keys from `SystemRandom`, hashes a key with SHA-256 and
compares it in constant time. The RS256 signature of a Google ID token is
verified by `google_cloud::IdTokenVerifier`.

## Context

A bot authenticates to `bot-dispatcher` with the ID token of its job's service account
or, launched locally, with a random key, which needs random bytes and a hash.

## Alternatives

- `sha2` and `rand`. Two new crates for what one already in the tree does.

## Decision

Use `aws-lc-rs`: `auth-token` already compiles it for HS256, and it covers
randomness, SHA-256 and constant-time comparison.

# base64 for JWT segments

## Status

Accepted

## Date

2026-11-18

## What

Add `base64` as a direct dependency of `bots`. `id_token.rs` decodes the
base64url segments of a Google ID token and the modulus and exponent of
Google's JWKs.

## Context

A JWT and a JWK carry their binary parts as unpadded base64url.

## Alternatives

- Expose the decoding in `auth-token`. Its JWT helpers are private to the
  HS256 client tokens, and an RS256 token is not one.
- Hand-written base64url. A few dozen lines the crate already gets right.

## Decision

Use `base64`, already in the workspace (`recorder`, `mediapack`).

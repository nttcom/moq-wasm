# `base64` for Google JWS and JWK Decoding (`google-cloud`)

**Status:** Accepted (2026-11-21)

## What

Add `base64` as a direct dependency of `google-cloud` to decode unpadded base64url segments of Google-signed JWTs and JWK coordinates/components.

## Context

IAP assertions, Google ID tokens, and Google's published JWK sets encode binary signatures, claims, P-256 coordinates, and RSA modulus/exponent values as unpadded base64url.

## Alternatives

- **Hand-written base64url decoding:** Unnecessary duplication when `base64` is already used across the workspace.

## Decision

Use `base64` alongside `aws-lc-rs` and `serde_json`.

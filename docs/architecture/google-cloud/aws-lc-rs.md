# aws-lc-rs for verifying Google-signed JWTs

## Status

Accepted

## Date

2026-11-21

## What

Add `aws-lc-rs` as a direct dependency of `google-cloud`. `jwt.rs` verifies ES256 IAP assertions (`ECDSA_P256_SHA256_FIXED`) and RS256 Google ID tokens (`RSA_PKCS1_2048_8192_SHA256`).

## Context

`console` verifies Google Cloud IAP assertions and `bot-dispatcher` verifies Google ID tokens from Cloud Run Job metadata servers. Both fetch Google JWKS sets and verify compact JWS signatures, so consolidating them in `google-cloud` gives both services a shared timeout-bounded, refresh-throttled JWKS cache.

## Alternatives

- Keep separate `aws-lc-rs` verification implementations in `console` and `bot-dispatcher`. Duplicates JWS parsing and JWKS caching, and allowed `bot-dispatcher` to drift without request timeouts or refresh throttling.
- Add `jsonwebtoken`. Introduces a new crate family while still needing custom JWKS refresh throttling.

## Decision

Use `aws-lc-rs`. `google-cloud`'s unconditional `reqwest` (`rustls`) dependency already builds `aws-lc-rs`, so no extra crypto stack is added.

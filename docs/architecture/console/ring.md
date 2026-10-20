# ring for ES256 verification of IAP assertions

## Status

Accepted

## Date

2026-10-20

## What

Add `ring` as a direct dependency of the `console` crate. `iap.rs` verifies
the ES256 signature of the IAP assertion with
`ring::signature::ECDSA_P256_SHA256_FIXED`; the tests sign assertions with
`EcdsaKeyPair`.

## Context

IAP signs the `x-goog-iap-jwt-assertion` header with ES256 and publishes its
public keys as P-256 JWKs. The console must check that signature before it
trusts the `sub` inside.

## Alternatives

- `jsonwebtoken`. Verifies ES256 and checks `exp`, `iss` and `aud`, but is a
  new crate in the tree whose key and validation types the code would still
  wrap for the JWK set and the 30-second skew.
- `p256` (RustCrypto). ECDSA over P-256 without the rest of a JWT library, but
  a new crate family next to the `ring` already compiled.

## Decision

Use `ring`. It is already compiled in the workspace for QUIC, TLS and the
HS256 tokens in `auth-token`, and verifying a fixed-length P-256 signature is
one call.

# aws-lc-rs for ES256 verification of IAP assertions

## Status

Accepted

## Date

2026-11-10

## What

Add `aws-lc-rs` as a direct dependency of the `console` crate. `apps.rs`
draws each app's secret from `SystemRandom`. The ES256 signature of the IAP
assertion is verified by `google_cloud::IapVerifier`
(see [google-cloud/aws-lc-rs.md](../google-cloud/aws-lc-rs.md)).

## Context

The console creates the signing secret of every app it registers, which needs
random bytes from the operating system.

## Alternatives

- `rand`. A new crate for what the crypto library already compiled does.
- `ring`, used here until 2026-11-10. Same API, but a second crypto library
  once the workspace's TLS moved to `aws-lc-rs`.

## Decision

Use `aws-lc-rs`. It is already compiled in the workspace for QUIC, TLS and the
HS256 tokens in `auth-token`.

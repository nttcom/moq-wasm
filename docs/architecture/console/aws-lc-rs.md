# `aws-lc-rs` for App Secret Generation in `console`

## Status
Accepted (2026-11-10)

## What
Direct dependency of `console`. `apps.rs` generates per-app signing secrets using `SystemRandom`. (IAP ES256 verification lives in `google_cloud::IapVerifier`; see [`google-cloud/aws-lc-rs.md`](../google-cloud/aws-lc-rs.md).)

## Context
Registering or rotating an app requires OS-backed cryptographic random bytes for its signing secret.

## Alternatives
- **`rand`**: Adds a new crate for randomness already provided by the crypto library.
- **`ring`** (used until 2026-11-10): Compiles a second crypto stack after workspace TLS migrated to `aws-lc-rs`.

## Decision
Use `aws-lc-rs`, already compiled for QUIC, TLS, and `auth-token`.

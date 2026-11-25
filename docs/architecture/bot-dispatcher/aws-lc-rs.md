# `aws-lc-rs` for Bot IDs and Local Bot Keys

## Status
Accepted (2026-11-18)

## What
Direct dependency of `bot-dispatcher`. `bots.rs` generates bot IDs and local bot keys via `SystemRandom`, hashes keys with SHA-256, and compares hashes in constant time. (Google ID token RS256 verification lives in `google_cloud::IdTokenVerifier`.)

## Context
Locally launched bots authenticate with a random key instead of a Google service-account ID token, requiring CSPRNG bytes, SHA-256, and constant-time comparison.

## Alternatives
- **`sha2` + `rand`**: Adds two crates for primitives already present in the tree.

## Decision
Use `aws-lc-rs`: already compiled via `auth-token` and provides randomness, SHA-256, and constant-time comparison in one crate.

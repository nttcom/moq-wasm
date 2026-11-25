# `aws-lc-rs` for Random Temporary App IDs (`temporary-apps`)

**Status:** Accepted (2026-11-10)

## What

Add `aws-lc-rs` as a direct dependency of `temporary-apps`. `issuer.rs` generates the 16 random bytes of every temporary `appId` via `aws_lc_rs::rand::SystemRandom`.

## Context

A temporary app's ID serves as its namespace root and must be unguessable; otherwise any client could subscribe under an app without holding its token.

## Alternatives

- **`rand` or `getrandom`:** Reads the OS CSPRNG equally well, but adds another crate when `aws-lc-rs` is already compiled for HS256 signing in `auth-token`.
- **A UUID crate:** Adds a dependency solely to hex-format bytes the service already formats directly.

## Decision

Use `aws-lc-rs`, matching `console`'s app ID and secret generation. It is already in the dependency tree and fills the 16 bytes in one call.

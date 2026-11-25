# `aws-lc-rs` for HMAC-SHA256

## Status
Accepted (2026-11-10)

## What
Direct dependency of `auth-token`. `jwt.rs` signs and verifies HS256 JWTs with `aws_lc_rs::hmac` (`HMAC_SHA256`, constant-time `verify`), pairing with `base64` for header/payload encoding.

## Context
Both `auth-token` CLI and `console` sign HS256 JWTs and VTS verifies them, so signing/verification lives beside `Claims` rather than inside VTS. Only HMAC-SHA256 + base64url is needed; the relay owns all claim policy.

## Alternatives
- **`jsonwebtoken`**: Validates `exp` by default (requiring opt-out because VTS passes time claims untouched); adds a new crate.
- **`hmac` + `sha2` (RustCrypto)**: Lock only holds older `sha2` 0.9 / `hmac` 0.10–0.12; adds a new compile unit and two crates.
- **`josekit` / `jwt-simple`**: Oversized new dependencies for a single algorithm.
- **`ring`** (used until 2026-11-10): Would compile a second crypto library now that workspace TLS uses `aws-lc-rs` ([`moqt/aws-lc-rs.md`](../moqt/aws-lc-rs.md)).

## Decision
Use `aws-lc-rs`: already compiled under `rustls` and `quinn` (zero new crates for `relay`), constant-time `verify`, and ~12 lines of code.

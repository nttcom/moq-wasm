# `thiserror` for `SignatureError`

## Status
Accepted (2026-10-20)

## What
Direct dependency of `auth-token` deriving `std::error::Error` and `Display` for `SignatureError`.

## Context
`verify_signature` distinguishes malformed tokens from invalid signatures so VTS maps them to `malformed_token` vs. `invalid_signature`. Workspace coding style requires a `thiserror` enum whenever callers branch on error variants.

## Alternatives
- **Hand-written `Display` / `Error`**: Extra boilerplate to keep synchronized with variants.
- **Plain non-`Error` enum**: Prevents `?` propagation into `anyhow::Result`.

## Decision
Use `thiserror`: already in the workspace lock (no new compile unit) and keeps error messages beside variants.

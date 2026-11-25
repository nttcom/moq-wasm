# `clap` for the `auth-token` CLI

## Status
Accepted (2026-10-22)

## What
Optional dependency of `auth-token` (`derive`, gated behind the `cli` feature) to parse arguments for the `auth-token` binary.

## Context
Operators, tests, and services mint JWTs from the CLI. Signing moved from `vts`'s `vts-mint` (which read the whole registry inside a verification-only service) into `auth-token`, taking only the secret.

## Alternatives
- **Separate CLI crate**: Unnecessary crate overhead for a few dozen lines wrapping `auth-token`.
- **Manual argument parsing**: Duplicates help text and error handling.
- **Non-optional dependency**: Forces every library consumer (including `relay`) to build `clap`.

## Decision
Use `clap` (`derive`) behind the optional `cli` feature: already in the workspace lock and excluded from library builds.

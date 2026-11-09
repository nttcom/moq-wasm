# ring for random temporary app IDs

## Status

Accepted

## Date

2026-11-05

## What

Add `ring` as a direct dependency of the `token-mint` crate.
`issuer.rs` draws the 16 random bytes of every temporary `appId` from
`ring::rand::SystemRandom`.

## Context

A temporary app's ID is its namespace root. It must not be guessable, or
anyone could subscribe under an app whose token they were never given.

## Alternatives

- `rand` or `getrandom`. Either reads the OS generator just as well, but is a
  new crate in this package while `ring` is already compiled for the HS256
  signing in `auth-token`.
- A UUID crate. Adds a dependency only to format bytes the code already
  formats as hex.

## Decision

Use `ring`, as `console` does for its app IDs and secrets: it is already in
the dependency tree and one call fills the bytes.

# reqwest for the VTS and Google's keys

## Status

Accepted

## Date

2026-11-18

## What

Add `reqwest` (`json`, `rustls`) as a direct dependency of `bots`.
`launch_auth.rs` posts launch tokens to the VTS and `id_token.rs` fetches
Google's JWKS; `job-launcher` and `google-cloud` take a client too.

## Context

`bots` calls two HTTP services and the crates it builds on take a
`reqwest::Client`.

## Alternatives

- `hyper`'s client. Lower level, and TLS would be wired by hand.

## Decision

Use `reqwest`, already used by `recorder`, `google-cloud` and
`job-launcher`.

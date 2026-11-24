# reqwest for the VTS and Google's keys

## Status

Accepted

## Date

2026-11-18

## What

Add `reqwest` (`json`, `rustls`) as a direct dependency of `bot-dispatcher`.
It builds the `reqwest::Client` that `google_cloud::MetadataToken` and
`job-launcher` take.

## Context

The crates `bot-dispatcher` builds on take a `reqwest::Client`. Launch tokens
go to the VTS through `auth_token::VtsClient`, and Google's JWKS is fetched by
`google_cloud::IdTokenVerifier`.

## Alternatives

- `hyper`'s client. Lower level, and TLS would be wired by hand.

## Decision

Use `reqwest`, already used by `recorder`, `google-cloud` and
`job-launcher`.

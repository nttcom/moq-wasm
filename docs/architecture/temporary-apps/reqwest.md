# reqwest for calling bot-dispatcher

## Status

Accepted

## Date

2026-11-18

## What

Add `reqwest` (`json`, `rustls`) as a direct dependency of `temporary-apps`;
it was a dev-dependency of the tests.

## Context

The service is the app server of temporary apps and calls `bot-dispatcher` over HTTPS
for them.

## Alternatives

- `hyper`'s client. Lower level, with TLS wired by hand.

## Decision

Use `reqwest`, already the workspace's HTTP client (`recorder`, `bot-dispatcher`,
`metadata-token`).

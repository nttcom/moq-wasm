# `reqwest` for Calling `bot-dispatcher` (`temporary-apps`)

**Status:** Accepted (2026-11-18)

## What

Promote `reqwest` (`json`, `rustls`) from a dev-dependency to a direct dependency of `temporary-apps`.

## Context

`temporary-apps` acts as the app server for temporary apps and calls `bot-dispatcher` over HTTPS on their behalf.

## Alternatives

- **`hyper` client:** Lower level; requires wiring TLS manually.

## Decision

Use `reqwest`, already the workspace's standard HTTP client (`recorder`, `bot-dispatcher`, `google-cloud`).

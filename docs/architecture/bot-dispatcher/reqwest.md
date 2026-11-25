# `reqwest` for Outbound Calls in `bot-dispatcher`

## Status
Accepted (2026-11-18)

## What
Direct dependency of `bot-dispatcher` (`json`, `rustls`) to construct the `reqwest::Client` passed to `google_cloud::MetadataToken` and `job-launcher` (VTS calls use `auth_token::VtsClient`; JWKS fetches use `google_cloud::IdTokenVerifier`).

## Context
Shared workspace libraries (`google-cloud`, `job-launcher`) accept a caller-owned `reqwest::Client`.

## Alternatives
- **`hyper` client**: Lower level; requires wiring TLS and connection pooling manually.

## Decision
Use `reqwest`, already used across `recorder`, `google-cloud`, and `job-launcher`.

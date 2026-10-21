# reqwest for the IAP public keys

## Status

Accepted

## Date

2026-10-20

## What

Add `reqwest` (with the `rustls` and `json` features, no default features) as
a direct dependency of the `console` crate. `iap.rs` uses it to fetch the IAP
public keys from `https://www.gstatic.com/iap/verify/public_key-jwk`.

## Context

The console verifies IAP assertions against keys that IAP rotates, so it
fetches the key set over HTTPS and caches it.

## Alternatives

- `hyper` as an HTTP client. Already a dependency for the server, but TLS and
  connection handling would be hand-written.

## Decision

Use `reqwest`. It is already in the workspace lock at 0.13.4 with the same
features (`relay`, `app-registry`), so no new crate enters the dependency
tree.

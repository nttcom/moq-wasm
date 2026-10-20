# reqwest for the Cloud Storage JSON API

## Status

Accepted

## Date

2026-10-20

## What

Add `reqwest` (with the `rustls` and `json` features, no default features) as a
direct dependency of the `app-registry` crate. `src/store.rs` uses it to read
the registry object, read its generation and upload it with
`ifGenerationMatch`; `src/credentials.rs` uses it to take an access token from
the metadata server.

## Context

The registry is one JSON object in Cloud Storage that the VTS reads and the
console writes. The crate needs three Cloud Storage calls and one metadata
server call, each a single HTTP request.

## Alternatives

- `google-cloud-storage` (official Rust client). Covers the whole Storage API
  and its own authentication, but brings a large new dependency tree for three
  requests.
- `object_store`. One API over Cloud Storage, S3 and local files, with
  conditional puts, which would fit an on-premises store later. New crate with
  its own HTTP and auth stack; no second store exists yet.
- `hyper` as an HTTP client. Already in the workspace for servers, but TLS,
  connection pooling and URL encoding would be hand-written.

## Decision

Use `reqwest` over the JSON API. It is already in the workspace lock at 0.13.4
(`relay`, `observability`, `onvif-ingest`), so no new crate enters the
dependency tree.

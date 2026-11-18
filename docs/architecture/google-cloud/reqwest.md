# reqwest for the Cloud Storage JSON API

## Status

Accepted

## Date

2026-10-20

## What

Add `reqwest` (with the `rustls` and `json` features, no default features) as a
direct dependency of the `google-cloud` crate. `src/cloud_storage.rs` uses it to
download an object with its generation and to upload or delete it with
`ifGenerationMatch`; `src/metadata_token.rs` uses it to take an access token
from the metadata server.

## Context

In production the app registry is one JSON object in Cloud Storage that the
VTS reads and the console writes, and the recorder releases a room's lock only
if no other request took it since. These need four Cloud Storage calls and one
metadata server call, each a single HTTP request. The decision was first made
for `app-registry`; the calls moved to `google-cloud` when the recorder needed
the same ones.

## Alternatives

- `google-cloud-storage` (official Rust client). Covers the whole Storage API
  and its own authentication, but brings a large new dependency tree for three
  requests.
- `object_store`. One API over Cloud Storage, S3 and local files, with
  conditional puts, which the recorder uses for its files. It has no
  conditional delete, and the registry's two stores are three requests to
  Cloud Storage and a few `tokio::fs` calls.
- `hyper` as an HTTP client. Already in the workspace for servers, but TLS,
  connection pooling and URL encoding would be hand-written.

## Decision

Use `reqwest` over the JSON API. It is already in the workspace lock at 0.13.4
(`relay`, `observability`, `onvif-ingest`), so no new crate enters the
dependency tree.

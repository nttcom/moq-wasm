# `object_store` for `bot-dispatcher` State

## Status
Accepted (2026-11-18)

## What
Direct dependencies `object_store` (`gcp`, `fs`) and `url` in `bot-dispatcher`. `store.rs` stores bot records, stop flags, and per-room holders in GCS or a local directory.

## Context
`bot-dispatcher` scales to zero and runs multiple instances, so state lives outside memory. Enforcing at most one active bot per `(type, room)` requires atomic create-if-absent and conditional replace writes.

## Alternatives
- **Database (Firestore / Cloud SQL)**: Operational overhead of another service for a handful of small JSON objects.
- **GCS JSON API over `reqwest`**: Requires maintaining auth, retries, and a local-filesystem backend manually.

## Decision
Use `object_store` (matching `recorder`): `PutMode::Create` / `PutMode::Update` map directly to GCS generation preconditions, and `file://` supports local development.

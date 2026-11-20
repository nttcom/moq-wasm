# object_store for bot-dispatcher's state

## Status

Accepted

## Date

2026-11-18

## What

Add `object_store` (`gcp`, `fs`) and `url` as direct dependencies of `bot-dispatcher`.
`store.rs` keeps the bot records, stop requests and room holders in a
Cloud Storage bucket or a local directory.

## Context

`bot-dispatcher` scales to zero and runs any number of instances, so its state lives
outside the process. One room runs at most one bot of each type, which needs
a create-if-absent and a replace-if-unchanged write.

## Alternatives

- A database (Firestore, Cloud SQL). Transactions, but a new service to run
  for a handful of small objects.
- The Cloud Storage JSON API on `reqwest`. The preconditions are reachable,
  but authentication, retries and the local directory would be the crate's
  own.

## Decision

Use `object_store`, as `recorder` does for the same kind of state:
`PutMode::Create` and `PutMode::Update` map onto Cloud Storage's generation
preconditions, and a `file://` directory serves development.

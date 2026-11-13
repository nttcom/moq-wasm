# serde_json for reading a catalog object

## Status

Accepted

## Date

2026-11-13

## What

Move `serde_json` from a dev-dependency to a dependency of the `msf` crate.
`Catalog::parse` uses it to read a catalog object and to refuse one whose
`version` is not `draft-01` or that is a delta update.

## Context

draft-ietf-moq-msf-01 §5.1.1: "A subscriber MUST NOT attempt to parse a
catalog version which it does not understand." Every reader (the recorder,
live-translate, live-egress, moq-cli and the browser through
`crates/wasm`) deserialized the catalog with `serde_json` itself and never
looked at `version`. A delta update (§5.1.6) carries no `version` (§5.3), so
the check needs the JSON document before it becomes a `Catalog`.

## Alternatives

- A `Catalog::check(&self)` the readers call after deserializing. Keeps `msf`
  free of `serde_json`, but leaves each reader to remember the call, and a
  delta update fails to deserialize before it can be told apart from a
  malformed object.
- A custom `Deserialize` that rejects the version. Hides the check inside
  serde and still cannot return why a catalog was refused without a typed
  error out of serde's error.

## Decision

Parse in `msf`, the crate that owns the catalog format, with `serde_json`.
Every reader already depends on it, so no new crate enters the dependency
tree.

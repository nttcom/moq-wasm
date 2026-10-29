# serde_json for custom catalog fields

## Status

Accepted

## Date

2026-10-29

## What

Make `serde_json` a normal dependency of the `msf` crate instead of a
dev-dependency. `Catalog` and `Track` gain a `custom_fields` map of
`serde_json::Value`s, flattened into the JSON object.

## Context

draft-ietf-moq-msf-01 §5 lets a producer add fields that do not collide with
the draft's field names. A parser must ignore the fields it does not
understand. The values are arbitrary JSON, so they need a JSON value type.
They must also be written into the catalog object itself. §5.1.7 requires `initDataList` to follow the `tracks`
array, and the workspace's `serde_json` does not preserve key order, so the
catalog cannot be serialized to a `serde_json::Value` and have fields merged
in afterwards.

## Alternatives

- Merge the fields into a `serde_json::Value` in `broadcast` after
  serializing the catalog. Keeps `msf` free of `serde_json`. But a `Value`
  object sorts its keys, which moves `initDataList` before `tracks`.
- Enable `serde_json`'s `preserve_order` feature to fix that order. It changes
  the key order of every `Value` in the workspace and makes `serde_json`
  depend on `indexmap`.
- A generic `Track<Extra>` parameter. Spreads a type parameter through every
  user of `msf` for one producer's need.

## Decision

Flatten a `serde_json::Map` into `Catalog` and `Track`. The known fields keep
their declaration order and the custom fields follow them. `serde_json` is
already in the workspace lock and was a dev-dependency of `msf`, so no new
crate enters the dependency tree. `msf` exports the draft's field names as
`ROOT_FIELD_NAMES` and `TRACK_FIELD_NAMES`, so a producer can reject a
colliding custom field.

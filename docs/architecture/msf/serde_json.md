# `serde_json` for Catalog Parsing (`msf`)

**Status:** Accepted (2026-11-13)

## What

Promote `serde_json` from a dev-dependency to a regular dependency of `msf`. `Catalog::parse` uses it to parse catalog objects and reject unsupported `version` values or delta updates.

## Context

`draft-ietf-moq-msf-02` §5.1.1 states: *"A subscriber MUST NOT attempt to parse a catalog version which it does not understand."* Previously, every consumer (`recorder`, `live-translate`, `egress-gateway`, `moq-cli`, and `sdk/js`) deserialized catalogs with `serde_json` directly without checking `version`. Because delta updates (§5.1.6) omit `version` (§5.3), validation must inspect the JSON document before constructing a `Catalog`.

## Alternatives

- **`Catalog::check(&self)` called after deserialization:** Keeps `serde_json` out of `msf`, but relies on every consumer to call it and lets delta updates fail deserialization before they can be distinguished from malformed JSON.
- **Custom `Deserialize` checking `version`:** Hides validation inside `serde` and loses typed refusal reasons inside `serde`'s error type.

## Decision

Centralize parsing and version validation in `msf::Catalog::parse` using `serde_json`. It is already in the workspace lock, adding no new crate to the dependency tree.

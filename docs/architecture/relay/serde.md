# serde for the VTS request and response bodies

## Status

Accepted

## Date

2026-09-09

## What

Add `serde` (with `derive`) as a direct dependency of the `relay` crate, and
`serde_json` as a dev-dependency. `serde` derives the request and response
structs exchanged with the Verify Token Service; `serde_json` is used only in
unit tests to build a `VerifyResponse` from a literal JSON document.

## Context

`reqwest`'s `json` feature serializes and deserializes bodies through `serde`,
so the relay needs `serde::Serialize` / `Deserialize` on its own types. The
response uses camelCase keys (`appId`, `isRelay`), handled by
`#[serde(rename_all = "camelCase")]`.

## Alternatives

- Build and parse the JSON by hand with `serde_json::Value`. Avoids the derive
  dependency but moves field names and types into string lookups.
- Send the token as a form body and parse the response manually. Same
  trade-off, plus a second wire format to document.

## Decision

Use `serde` derives. Both crates are already in the workspace lock (`moqt` and
`bindings/wasm` depend on `serde`; `serde_json` is used by the bridges and
pulled in by `reqwest/json`), so no new crate enters the dependency tree.
`serde_json` stays a dev-dependency because production code only touches JSON
through `reqwest`.

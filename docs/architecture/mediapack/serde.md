# serde for the LOC extension boundary with JavaScript

## Status

Accepted

## Date

2026-09-18

## What

Add `serde` (with `derive`) as an optional dependency of the `mediapack`
crate behind a `serde` feature, and enable the `serde` feature of `bytes` with
it. The feature derives `Serialize` / `Deserialize` for `loc::LocExtension` and
`loc::LocValue`; `serde_json` is a dev-dependency that pins the JSON shape in a
unit test.

## Context

`bindings/wasm` hands LOC header extensions across the JavaScript boundary
with `serde_wasm_bindgen`, and previously did so through a parallel set of LOC
types in the `packages` crate. Folding `packages` into `mediapack` leaves the
`mediapack` types as the only LOC model, so they must be the ones that cross
the boundary. The shape is `{ id, value: { varint } | { bytes } }`, which is
the draft-ietf-moq-loc-01 §2.3 representation and is mirrored by
`examples/browser/utils/media/loc.ts`.

## Alternatives

- Keep a serde-only DTO enum in `bindings/wasm` and convert to and from
  `mediapack::loc::LocExtension`. Keeps `mediapack` free of `serde`, but
  keeps the id-to-name table that the merge is removing alive in a second
  place.
- Build the JavaScript objects by hand with `js_sys`. Avoids `serde`, but
  moves field names into string literals on both sides of the boundary.

## Decision

Derive `serde` behind an opt-in feature so native consumers (`live-ingest`,
`transcode`, `bindings/gstreamer`) do not pull it in. `serde` and `serde_json`
are already in the workspace lock, so no new crate enters the dependency tree.

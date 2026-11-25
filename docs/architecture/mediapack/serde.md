# `serde` for the JavaScript LOC Extension Boundary (`mediapack`)

**Status:** Accepted (2026-09-18)

## What

Add optional `serde` (`derive`) behind a `serde` feature on `mediapack`, enabling `bytes/serde` alongside it:

- Derives `Serialize`/`Deserialize` for `loc::LocExtension` and `loc::LocValue`.
- Adds `serde_json` as a dev-dependency to pin the JSON wire shape in unit tests.

## Context

`sdk/js` passes LOC properties across the JS boundary with `serde_wasm_bindgen`. After folding the old `packages` crate into `mediapack`, `mediapack` owns the single LOC model. The serialized shape `{ id, value: { varint } | { bytes } }` matches `draft-ietf-moq-loc-04` §2.3 and mirrors `sdk/js/js/src/media/internal/loc.ts`.

## Alternatives

- **Separate serde DTO enum in `sdk/js`:** Keeps `serde` out of `mediapack`, but perpetuates a duplicate ID-to-name mapping.
- **Manual JS object construction via `js_sys`:** Avoids `serde`, but duplicates field names as string literals on both sides of the boundary.

## Decision

Derive `serde` behind an opt-in `serde` feature so native consumers (`ingest-gateway`, `transcode`, `crates/gst-plugin-moqt`) do not compile it. `serde` and `serde_json` are already in the workspace lock, adding no new crate to the dependency tree.

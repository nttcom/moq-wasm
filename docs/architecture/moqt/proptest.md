# `proptest` for Codec and Protocol Property Tests (`moqt`)

**Status:** Accepted (2026-11-19)

## What

Add `proptest` (`default-features = false`, feature: `std`) as a dev-dependency of `moqt` to verify wire codecs and protocol state machines over generated inputs instead of only hand-picked examples.

### Properties Tested

**Wire framing and varints**
- Draft-14 control messages round-trip identically and consume every encoded byte.
- Varints of either draft decode identically across arbitrary byte-chunk splits.
- Framed control streams, subgroup streams, and FETCH responses of either draft decode identically across arbitrary chunk splits.

**Draft-18 and cross-version relaying**
- Received draft-18 Track Properties re-encode byte-identically regardless of integer encoding width; calling setters re-encodes only modified properties.
- Draft-18 `SUBSCRIBE`, `SUBSCRIBE_NAMESPACE`, `PUBLISH`, `FETCH`, and `PUBLISH_DONE` round-trip up to draft-18 limits (`FETCH` reads publisher group order as `Ascending`; reason phrases truncate at 1,024 bytes) and fail to encode only for a `DELETE` auth token or an inverted range.
- Sequences of model `SUBSCRIBE_UPDATE`s encoded as draft-18 `REQUEST_UPDATE`s match the receiver baseline state; received Subscription Filters cover the same groups.
- Cross-draft forwarding (`SUBSCRIBE`, `TRACK_STATUS`, `SUBSCRIBE_NAMESPACE`, `PUBLISH_NAMESPACE`, `PUBLISH`, `PUBLISH_DONE`, `FETCH`) preserves meaning up to draft-omitted fields (reason phrases truncated at 1,024 bytes on a UTF-8 boundary, fields absent in draft-14 `PUBLISH`, `PUBLISH_DONE` codes undefined in draft-14), failing to encode only at exact draft limits (draft-14: empty namespaces or integers `>= 2^62`; draft-18: empty namespace fields, `DELETE` tokens, or inverted ranges).

**Session state**
- During session migration, `GroupCutover` delivers each group from at most one subscription (all objects or none) and starts from the current subscription only at group start while the previous subscription may still deliver it; once fixed (and signalled exactly then), the cutover lies above every group it counts as taken from the previous subscription and at or below every group taken from the current one.
- `UpdateWaiters` hands every REQUEST_UPDATE answer to the caller of the update it answers when updates are written or withdrawn one at a time.
- `ObjectLedger` rejects an object copy if and only if it conflicts with an accepted copy, making malformation detection independent of arrival order.

Shared strategies live in `modules/test_support/control_message_strategies.rs` and `modules/test_support/object_strategies.rs`. They bias toward encoding boundaries (varint width edges, object/group gaps of `0..=3` where deltas are omitted or shortened) that uniform random generation rarely hits.

## Context

Past `moqt` codec bugs occurred on inputs no example test listed: varint width edges, names/reason phrases at draft length limits, lengths exceeding 32 bits on `wasm32`, and non-UTF-8 fields. `wire_noise()` checks panic/stall safety on malformed bytes, not round-trip fidelity on valid messages. Property testing explores field combinations automatically and shrinks failures to minimal counterexamples.

## Alternatives

- **`quickcheck`:** Ties generation to one `Arbitrary` impl per type, requiring newtypes and manual constraint-preserving `shrink` impls for draft bounds (1–32 namespace fields, 4,096-byte Full Track Name).
- **`bolero` / `cargo-fuzz`:** Better for crash-finding fuzzing, but needs nightly Rust and runs outside `cargo test`; not a replacement for round-trip tests (can be added separately later).
- **More example tests:** Only checks values explicitly anticipated by the author.

## Decision

Use `proptest`. Combinator strategies encode draft constraints once and preserve them during shrinking. Tests run under `cargo test` on stable (256 cases per property by default; override via `PROPTEST_CASES`). With default features off, it adds four crates to the lock (`proptest`, `rand_xorshift`, `unarray`, `core_detect`) and never propagates to downstream crates.

Failing cases are written to `crates/moqt/proptest-regressions/` and replayed first on the next run; commit regression files alongside their fixes.

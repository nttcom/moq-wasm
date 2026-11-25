# `proptest` for Cache Range Property Tests (`relay`)

**Status:** Accepted (2026-11-19)

## What

Add `proptest` (`default-features = false`, feature: `std`) as a dev-dependency of `relay` to test cache known-position bookkeeping against brute-force reference models:

- **`KnownRanges`:** After arbitrary range insertions and removals, `covers_until` and `end_of_range_containing` match a position set, and stored ranges remain disjoint and non-adjacent.
- **Upstream FETCH ingestion:** An ascending upstream FETCH response records its full range minus any stretches marked unknown.
- **`plan_fetch`:** Plans exactly the unexpired cached objects in range (in either group order) and emits one unknown stretch through the position before the next object for every unknown run.

Shared generator `test_support::location_strategies` models each group with Object IDs `0..=5`, `u64::MAX`, and one stand-in for intermediate IDs, with groups starting at `0` or ending at `u64::MAX` (covering draft-18 ranges ending past the last `Location`).

## Context

Cache-served FETCH responses emit unknown stretches for unknown positions and omit known empty positions (telling subscribers no object exists there). Any off-by-one at a range end or group boundary either hides an object or reports a bogus gap. Combinations of group boundaries, adjacent ranges, expired objects, and descending order outgrow hand-written examples.

## Alternatives

- **Exhaustive loops over a tiny domain:** No shrinking, and fast domains are too small to fit an expired object between two known ranges across three groups.
- **`cargo-fuzz`:** Requires nightly Rust and runs outside `cargo test`.
- **More example tests:** Only checks layouts explicitly written by the author.

## Decision

Use `proptest`, matching `moqt` and `mediapack`. It runs in `cargo test` on stable (256 cases per property by default; override via `PROPTEST_CASES`) and stays out of the `relay` binary.

Failing cases are saved under `services/relay/proptest-regressions/` and replayed first on the next run; commit regression files alongside their fixes.

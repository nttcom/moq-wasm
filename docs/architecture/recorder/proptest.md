# `proptest` for FETCH Range Property Tests (`recorder`)

**Status:** Accepted (2026-11-19)

## What

Add `proptest` (`default-features = false`, feature: `std`) as a dev-dependency of `recorder` to verify replay FETCH range matching:

- `in_fetch_range` includes exactly the locations specified by `[start, end)` in `draft-14` §9.16.1 and §9.16.3: from Start Location through the object before End Location, or through the entire group when End Location's Object ID is `0`.

The generator biases toward edge cases—`end` in the group before, equal to, or after `start`, at object `0`, `1`, or adjacent to `start`, plus group/object IDs `0` and `u64::MAX`—and checks every location adjacent to `start` and `end`.

## Context

Replay reads archived objects from storage rather than a track cache, so it uses its own range check instead of `moqt`'s. Range bugs typically appear at single boundary conditions (inclusive ends, the whole-group `object == 0` rule, same-group bounds) that fixed examples miss; comparing generated ranges against a spec-derived model catches them every run.

## Alternatives

- **More example tests:** Only checks ranges explicitly listed by the author.
- **Exhaustive grid over small IDs:** Misses `u64::MAX` and widely separated IDs, and provides no shrinking on failure.

## Decision

Use `proptest`, matching `moqt` and `mediapack`. It runs in `cargo test` on stable (256 cases per property by default; override via `PROPTEST_CASES`) and stays out of the `recorder` binary.

Failing cases are saved under `services/recorder/proptest-regressions/` and replayed first on the next run; commit regression files alongside their fixes.

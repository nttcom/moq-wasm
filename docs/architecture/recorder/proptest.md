# proptest for property-based FETCH range tests in recorder

## Status

Accepted

## Date

2026-11-19

## What

Add `proptest` as a dev-dependency of the `recorder` crate, with default
features off and only `std` on. The tests use it to state one property of the
replay's FETCH handling:

- `in_fetch_range` holds exactly the locations a FETCH of `[start, end)`
  names under the draft text: from Start Location through the object before
  End Location, or through the whole group when End Location's Object is 0
  (draft-14 §9.16.1, §9.16.3).

The generator favours the boundaries a request reaches: an end in the group
before, of or after the start, at object 0, 1 or one off the start's, and
group and object IDs of 0 and `u64::MAX`. The property checks every location
next to the start and the end, where the answer can change.

## Context

The replay picks the archived objects a FETCH returns with its own range
check instead of `moqt`'s, because it reads them from storage rather than
from a track's cache. Its example tests check a few hand-picked ranges. A
range check fails at a single boundary (an inclusive end, the whole-group
rule, the same group as the start), which a property over generated ranges
compared with a model written from the draft reaches every run.

## Alternatives

- More example tests: each one covers only the ranges written in it.
- Loop over a fixed grid of starts and ends: covers small IDs exhaustively but
  not `u64::MAX` or IDs far apart, and does not shrink a failure.

## Decision

Use `proptest`, as `moqt` and `mediapack` do. It runs in `cargo test` on
stable (256 cases per property by default, set by `PROPTEST_CASES`), and as a
dev-dependency it reaches no crate that depends on `recorder`.

A failing case is written to `services/recorder/proptest-regressions/` and
replayed first on the next run; commit that file with the fix of the failure
it found.

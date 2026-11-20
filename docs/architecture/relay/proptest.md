# proptest for property-based cache tests in relay

## Status

Accepted

## Date

2026-11-19

## What

Add `proptest` as a dev-dependency of the `relay` crate, with default
features off and only `std` on. The tests use it to check the cache's
bookkeeping of known positions against brute-force models:

- after any sequence of inserted and removed ranges, `KnownRanges` answers
  `covers_until` and `end_of_range_containing` as a set of known positions
  does, and its ranges stay disjoint and never adjacent;
- the known ranges an ascending upstream FETCH response leaves are its range
  less the stretches it marked unknown;
- `plan_fetch` plans exactly the unexpired cached objects in the range, in
  either group order, with one unknown stretch through the position before
  the next object for each run of positions the cache does not know.

`test_support::location_strategies` generates the positions, shared by the
three. Each group has the Object IDs 0 to 5, `u64::MAX`, and one stand-in for
every ID between, so a model over a few dozen positions decides every
position, including the last of a group. The groups start at Group ID 0 or
end at `u64::MAX`, which a draft-18 publisher can send, so a range also ends
past the last Location.

## Context

A FETCH served from the cache reports every position it does not know as
unknown and says nothing about a position it knows, which tells the
subscriber that no object is there. An off-by-one in a range end or a group
boundary therefore either hides an object or reports a gap that is not
there. The example tests check a few hand-picked layouts; the bounds that
meet at a group's first or last object, adjacent ranges, expired objects and
descending order combine in more ways than they list.

## Alternatives

- Exhaustive loops over every layout of a smaller domain: no shrinking, and
  the domain that stays fast is too small to hold an expired object between
  two known ranges in three groups.
- `cargo-fuzz`: needs nightly and runs outside `cargo test`.
- More example tests: each one covers only the layout written in it.

## Decision

Use `proptest`, as `moqt` and `mediapack` do. It runs in `cargo test` on
stable (256 cases per property by default, set by `PROPTEST_CASES`), and as a
dev-dependency it adds nothing to the relay binary.

A failing case is written to `services/relay/proptest-regressions/` and
replayed first on the next run; commit that file with the fix of the failure
it found.

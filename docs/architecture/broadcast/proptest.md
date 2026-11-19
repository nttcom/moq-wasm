# proptest for property-based group alignment tests in broadcast

## Status

Accepted

## Date

2026-11-19

## What

Add `proptest` as a dev-dependency of the `broadcast` crate, with default
features off and only `std` on. The tests use it to state one property of the
group ids a writer gives its tracks: fed any interleaving of source video
frames, frames of a rendition that follows the source's keyframes, and audio
samples, each with timestamps that may repeat,

- every track's group ids increase, so `Origin` accepts each group a track
  starts;
- every track takes only the group ids the source video assigned, and a
  rendition keyframe on a source keyframe `GroupAlignment` still remembers
  takes that keyframe's id;
- each CMAF fragment starts the group its LOC object started
  (draft-ietf-moq-cmsf-01 §3.2).

## Context

The group ids come from `GroupAlignment`, which the source video, its
renditions and the audio reach in whatever order their samples arrive, and the
fMP4 muxer holds each video sample back one sample and moves one whose decode
time repeats. The example tests check single orders; a CMAF keyframe whose
decode time repeated took a group id of its own, which no example reached. A
property over random interleavings and timestamp gaps of a few muxer ticks
reaches those orders and shrinks a failure to a few samples.

## Alternatives

- More example tests, one per order: each covers the order its author thought
  of, which is how the repeated decode time was missed.
- A hand-written loop over generated orders: repeats what `proptest` does
  without shrinking a failure.

## Decision

Use `proptest`, as `moqt` and `mediapack` do. It runs in `cargo test` on
stable (256 cases per property by default, set by `PROPTEST_CASES`), and as a
dev-dependency it reaches no crate that depends on `broadcast`.

A failing case is written to `crates/broadcast/proptest-regressions/` and
replayed first on the next run; commit that file with the fix of the failure
it found.

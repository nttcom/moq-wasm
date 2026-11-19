# proptest for property-based demuxer tests in mediapack

## Status

Accepted

## Date

2026-11-19

## What

Add `proptest` as a dev-dependency of the `mediapack` crate, with default
features off and only `std` on. The tests use it to state two properties of
the byte-stream demuxers:

- the MPEG-TS, FLV, fragmented MP4 and Annex-B demuxers report the same
  events from a fixture however it is cut into chunks, and the ADTS reader
  reads back generated frames however their stream is cut;
- demuxing what the MPEG-TS and RTP muxers wrote gives back generated video
  and audio samples, with their timestamps, and the RTP demuxer gives them
  back when adjacent packets swap.

`test_support::cuts` and `test_support::push_in_chunks` cut the input and feed
it to a demuxer, shared by the tests of each format.

## Context

A demuxer receives a stream in chunks of whatever size the network or file
reads give it, so every boundary in a container can fall inside a chunk or
between two. The example tests feed a fixture whole or in one fixed chunk
size, which reaches few of those boundaries. A property over random cut
points reaches them all and shrinks a failure to a single cut.

## Alternatives

- Feed every fixture at every single cut point in a loop: covers one cut
  exhaustively but not two or more, and repeats the same loop per format.
- `cargo-fuzz`: finds crashes on malformed input, but needs nightly and runs
  outside `cargo test`; it does not check that well-formed input gives the
  same events.

## Decision

Use `proptest`, as `moqt` does for its codec. It runs in `cargo test` on
stable (256 cases per property by default, set by `PROPTEST_CASES`), and as a
dev-dependency it reaches no crate that depends on `mediapack`.

A failing case is written to `crates/mediapack/proptest-regressions/` and
replayed first on the next run; commit that file with the fix of the failure
it found.

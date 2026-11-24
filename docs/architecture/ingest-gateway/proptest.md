# proptest for the arrival clamp property in ingest-gateway pull

## Status

Accepted

## Date

2026-11-19

## What

Add `proptest` as a dev-dependency of the `ingest-gateway` crate, with default
features off and only `std` on. The tests use it to state one property of
`ArrivalClamp`: for a video and an audio track interleaved in presentation
order, with packet durations from 1 to 40 ms, arrival jitter up to 30 ms and a
first sample up to 500 ms late,

- each track's presentation times strictly increase, and
- once a track's shift has settled, no sample's capture time lies after its
  arrival.

## Context

`ArrivalClamp` moves the presentation times of both tracks towards the
earliest arrival seen. Whether a track keeps moving forward depends on how
its packet duration compares with the other track's and with the correction
step, and on how much later than the rest the first sample arrived. The
example tests fix one combination each; a bug that stalled 10 ms audio and
ran 30 fps video backwards next to 5 ms audio went unnoticed because no test
combined those durations. A property over generated durations, weighted to
the ones cameras send (audio 5, 10 and 20 ms, video 33.3 and 40 ms), reaches
the combinations and shrinks a failure to a short session.

## Alternatives

- Loop over a fixed table of audio and video durations: covers the listed
  pairs, but not jitter or the lateness of the first sample.
- `cargo-fuzz`: needs nightly and runs outside `cargo test`.

## Decision

Use `proptest`, as `moqt` and `mediapack` do. It runs in `cargo test` on
stable (256 cases per property by default, set by `PROPTEST_CASES`), and as a
dev-dependency it does not reach the `ingest-gateway` binary.

A failing case is written to `services/ingest-gateway/proptest-regressions/`
and replayed first on the next run; commit that file with the fix of the
failure it found.

# `proptest` for `ArrivalClamp` Properties (`ingest-gateway`)

**Status:** Accepted (2026-11-19)

## What

Add `proptest` (`default-features = false`, feature: `std`) as a dev-dependency of `ingest-gateway` to verify `ArrivalClamp` across generated interleaved video/audio streams (packet durations `1..=40 ms`, arrival jitter up to `30 ms`, first sample up to `500 ms` late):

- Each track's presentation timestamps strictly increase.
- Once a track's shift settles, no sample's capture time lies after its arrival time.

## Context

`ArrivalClamp` pulls both tracks' presentation timestamps toward the earliest observed arrival. Whether timestamps stay monotonic depends on the ratio between audio/video packet durations, the correction step, and initial sample lateness.

Single-case unit tests missed a bug that stalled 10 ms audio and moved 30 fps video backward alongside 5 ms audio. Property testing over generated durations—weighted toward real camera intervals (audio: 5, 10, 20 ms; video: 33.3, 40 ms)—exercises these combinations and shrinks failures to minimal sessions.

## Alternatives

- **Fixed table of audio/video durations:** Covers only listed pairs; omits jitter and initial sample lateness.
- **`cargo-fuzz`:** Requires nightly Rust and runs outside `cargo test`.

## Decision

Use `proptest`, matching `moqt` and `mediapack`. It runs in `cargo test` on stable (256 cases per property by default; override via `PROPTEST_CASES`) and stays out of the `ingest-gateway` binary.

Failing cases are saved under `services/ingest-gateway/proptest-regressions/` and replayed first on subsequent runs; commit regression files alongside their fixes.

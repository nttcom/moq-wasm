# `proptest` for Demuxer Property Tests (`mediapack`)

**Status:** Accepted (2026-11-19)

## What

Add `proptest` (`default-features = false`, feature: `std`) as a dev-dependency of `mediapack` to check two byte-stream demuxer properties:

- **Chunk-boundary invariance:** MPEG-TS, FLV, fragmented MP4, and Annex-B demuxers emit identical events from a fixture regardless of how input bytes are chunked; the ADTS reader recovers generated frames across arbitrary splits.
- **Mux/demux round-trip:** Demuxing output from the MPEG-TS and RTP muxers recovers generated video/audio samples and timestamps, including when adjacent RTP packets are reordered.

Shared helpers `test_support::cuts` and `test_support::push_in_chunks` split and feed inputs across all format tests.

## Context

Network and file reads deliver streams in arbitrary chunk sizes, so container boundaries can land inside or between chunks. Fixed-chunk unit tests miss most split positions; randomized cuts explore them and shrink failures to a single split.

## Alternatives

- **Exhaustive single-cut loop per fixture:** Covers one cut point, not multi-chunk splits, and duplicates loop boilerplate per format.
- **`cargo-fuzz`:** Targets crash finding on malformed input, requires nightly Rust, runs outside `cargo test`, and does not assert event equivalence on valid streams.

## Decision

Use `proptest`, matching `moqt`. It runs in `cargo test` on stable (256 cases per property by default; override via `PROPTEST_CASES`) and never propagates to downstream crates.

Failing cases are written to `crates/mediapack/proptest-regressions/` and replayed first on the next run; commit regression files alongside their fixes.

# [S] Unwrap the 33-bit MPEG-TS timestamps in the demuxer

Next: extend each PES PTS and DTS past the last one before `Timestamp::from_ticks` in `crates/mediapack/src/mpegts/demuxer.rs:217`.

## Goal

Timestamps keep rising when a stream's 33-bit PTS/DTS wraps (every 2^33 ticks at 90 kHz, about 26.5 h, sooner when the encoder starts high); today they jump back by 95,443 s.
Evidence: the round-trip property `mpegts::muxer::tests::properties::demuxing_muxed_samples_gives_them_back` fails once its start range reaches `(1 << 33) - 50_000`.
Done when that property passes with starts up to `1 << 33` and `live-ingest` SRT input crosses the wrap without a backward jump.

## Plan

Open: whether video and audio share one unwrap reference, since the two PIDs wrap at different packets.

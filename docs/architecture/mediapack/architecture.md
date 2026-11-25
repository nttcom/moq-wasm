# `mediapack` Architecture

See [`crates/mediapack/README.md`](../../../crates/mediapack/README.md) for supported formats, the `MediaEvent` model, and module layout.

## Muxers Kept Ahead of Their Callers

`flv::Muxer`, `mpegts::Muxer`, and `Transmuxer` currently have no callers in workspace services:
- Ingest bridges only demux FLV and MPEG-TS.
- `recorder` only writes fragmented MP4.

These muxers remain in the crate because FLV and MPEG-TS output is planned.

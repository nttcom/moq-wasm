# shared

Shared types used by multiple crates, bridges, and browser examples.

- `media-streaming-format/`: MSF catalog structures
- `mediapack/`: container demuxers/muxers (MPEG-TS, FLV, fMP4, LOC) and H.264/AAC bitstream helpers; the `moqt` feature maps LOC extensions to MoQT extension headers and the `serde` feature exposes them to JavaScript
- `transcode/`: GStreamer-backed re-encoding of `MediaEvent` video into multiple renditions

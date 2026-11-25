# GStreamer for Multi-Rendition Transcoding (`transcode`)

**Status:** Accepted (2026-09-08)

## What

Add `gstreamer` and `gstreamer-app` to `crates/transcode` to decode incoming H.264 access units and re-encode them into multiple resolutions and bitrates in-process.

## Context

`ingest-gateway` publishes the source rendition as received. Viewers on constrained links need lower-bitrate renditions, which requires decoding and re-encoding video. `mediapack` intentionally stays codec-free (pure-Rust containers and bitstream syntax only), so transcoding lives in its own crate with a `MediaEvent`-in / `MediaEvent`-out boundary.

## Alternatives

- **Pure-Rust / lightweight codecs:** `openh264` supports only baseline H.264 encoding via C bindings, `rav1e` is too slow for real time, and neither provides hardware acceleration.
- **`ffmpeg-next`:** Requires manually driving decode, scale, and N encode pipelines (or a `libavfilter` graph) plus per-platform hardware encoder glue.
- **External `ffmpeg`/`gst-launch` processes feeding multiple SRT streams:** Avoids Rust dependencies, but adds process management and an extra SRT hop per rendition.

## Decision

Use GStreamer Rust bindings: `tee` fan-out to N `videoscale ! x264enc` branches is built in with native back-pressure, and swapping `x264enc` for `vtenc_h264`/`nvh264enc` later requires only changing the pipeline description. The dependency stays isolated in `crates/transcode`, so `mediapack` and bridges compile without GStreamer unless they opt in.

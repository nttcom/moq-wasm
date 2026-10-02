# GStreamer for multi-rendition transcoding

## Status

Accepted

## Date

2026-09-08

## What

Add the `gstreamer` and `gstreamer-app` crates to the new `transcode` crate.
They decode the incoming H.264 access units and re-encode them into several
resolutions and bitrates in-process.

## Context

The live-ingest bridge publishes the source rendition as received. Viewers on
constrained links need lower renditions, which requires decoding and
re-encoding video. `mediapack` deliberately stays codec-free (containers and
bitstream syntax only, pure Rust), so the encoder dependency needs its own
crate with a `MediaEvent`-in / `MediaEvent`-out boundary.

## Alternatives

- Pure Rust codecs. `openh264` offers decoding and baseline-only encoding
  through bindings, `rav1e` is far from real time; neither gives a hardware
  path.
- `ffmpeg-next` (already used by the ONVIF bridge). Decoding, scaling and
  encoding N outputs must be driven by hand or through a libavfilter graph;
  hardware encoders need per-platform code.
- An external ffmpeg/gst-launch process feeding the bridge with several SRT
  streams. Zero new Rust dependencies, but adds a process to manage and an
  extra SRT hop per rendition.

## Decision

Use the GStreamer Rust bindings. `tee` fan-out to N `videoscale ! x264enc`
branches is a pipeline primitive, back-pressure is built in, and the same
description can swap `x264enc` for `vtenc_h264`/`nvh264enc` later. The
dependency is confined to `shared/transcode`; `mediapack` and the bridges
compile without GStreamer unless they opt in.

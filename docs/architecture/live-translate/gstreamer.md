# GStreamer for Opus decoding and encoding in the translator

## Status

Proposed

## Date

2026-10-06

## What

Use `gstreamer` and `gstreamer-app` in the `live-translate` crate. One pipeline
decodes the broadcaster's Opus packets to 16 kHz mono PCM for Gemini; another
encodes Gemini's 24 kHz mono PCM into 20 ms Opus packets for the translated
tracks.

## Context

The translator sits between Opus on MoQT and raw PCM on the Gemini Live API in
both directions, and needs resampling (48 kHz to 16 kHz) on the way in. The
workspace already depends on GStreamer 0.25 for `crates/transcode` and
`crates/gst-plugin-moqt`, and CI installs `gstreamer1.0-plugins-base`, which
contains `opusdec`, `opusenc`, `audioconvert` and `audioresample`.

## Alternatives

- `opus` (libopus bindings) plus a Rust resampler such as `rubato`. Lighter at
  run time, but two new crates for a job the existing dependency already does.
- Pure Rust Opus. Decoders exist; there is no maintained pure Rust encoder.

## Decision

Use GStreamer, driven through `appsrc` / `appsink` like `crates/transcode`.
Packet timestamps are not taken from GStreamer: the translator stamps encoded
packets in the order it pushed their frames, because GStreamer re-times buffers
around the timestamp jumps that silence produces.

## Consequences

### Positive
- No new crates; decoding, downmixing and resampling are one pipeline string.

### Negative
- The container image needs the GStreamer runtime and base plugins, so it
  cannot use the distroless base the other services use.
- `opusenc` holds back the last frame until the next one arrives, so the final
  20 ms of each translated utterance is published late and usually dropped by
  the viewer.

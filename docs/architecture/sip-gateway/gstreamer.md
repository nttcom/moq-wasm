# GStreamer for Opus and G.711 transcoding

## Status

Accepted

## Date

2026-11-20

## What

Add `gstreamer` and `gstreamer-app` 0.25 as dependencies of `sip-gateway`.
`media/transcoder.rs` runs two `appsrc` to `appsink` pipelines per call:
G.711 at 8 kHz to 20 ms Opus frames at 48 kHz (`mulawdec`/`alawdec`,
`audioresample`, `opusenc`), and Opus to G.711 (`opusdec`, `audioresample`,
`mulawenc`/`alawenc`).

## Context

A page publishes and plays Opus at 48 kHz; a trunk sends and takes G.711
at 8 kHz. Every call transcodes both ways and resamples by 6.

## Alternatives

- `audiopus` plus hand-written G.711 tables and a resampler. Three pieces,
  one a C binding of its own, for what GStreamer already does.
- Publishing G.711 into MoQT unchanged. Saves one direction's transcode but
  leaves every subscriber (players, `recorder`, `live-translate`) to handle
  a second audio codec, and PCMU unsupported.

## Decision

Use GStreamer, which `transcode` and `live-translate` already link, with
the same `appsrc`/`appsink` shape as `live-translate`'s Opus codec.

# GStreamer for Opus and G.711 Transcoding (`sip-gateway`)

**Status:** Accepted (2026-11-20)

## What

Add `gstreamer` and `gstreamer-app` 0.25 to `sip-gateway`. `media/transcoder.rs` runs two `appsrc` → `appsink` pipelines per call:

- **Trunk → MoQT:** G.711 (8 kHz) → 20 ms Opus (48 kHz) via `mulawdec`/`alawdec ! audioresample ! opusenc`.
- **MoQT → Trunk:** Opus (48 kHz) → G.711 (8 kHz) via `opusdec ! audioresample ! mulawenc`/`alawenc`.

## Context

Browser clients publish and play 48 kHz Opus, whereas SIP trunks send and expect 8 kHz G.711. Every call must transcode both directions and resample by a factor of 6.

## Alternatives

- **`audiopus` + custom G.711 tables + resampler:** Three separate pieces (including a new C binding) duplicating what GStreamer already does.
- **Publish G.711 directly onto MoQT:** Eliminates one transcode direction, but forces every subscriber (browsers, `recorder`, `live-translate`) to support a second codec (with PCMU unsupported).

## Decision

Use GStreamer, matching `transcode` and `live-translate`'s `appsrc`/`appsink` pattern.

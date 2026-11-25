# GStreamer for Opus Decode/Encode (`live-translate`)

**Status:** Accepted (2026-10-06)

## What

Use `gstreamer` and `gstreamer-app` in `live-translate`:

- **Input pipeline:** decodes incoming Opus packets and resamples (48 kHz → 16 kHz mono PCM) for Gemini.
- **Output pipeline:** encodes Gemini's 24 kHz mono PCM into 20 ms Opus packets for translated MoQT tracks.

## Context

The translator bridges Opus on MoQT and raw PCM on the Gemini Live API in both directions. The workspace already depends on GStreamer 0.25 (`crates/transcode`, `crates/gst-plugin-moqt`), and CI installs `gstreamer1.0-plugins-base`, which provides `opusdec`, `opusenc`, `audioconvert`, and `audioresample`.

## Alternatives

- **`opus` (`libopus` bindings) + `rubato` resampler:** Lighter runtime footprint, but adds two new crates for functionality GStreamer already supplies.
- **Pure-Rust Opus:** Decoders exist, but there is no maintained pure-Rust encoder.

## Decision

Use GStreamer via `appsrc`/`appsink` (matching `crates/transcode`). Stamp encoded packet timestamps in push order rather than reading them from GStreamer, because GStreamer re-times buffers across timestamp jumps caused by silence.

## Consequences

- **Positive:** No new crates; decoding, downmixing, and resampling are expressed as one pipeline string.
- **Negative:** Container images require the GStreamer runtime and base plugins (cannot use the distroless base image other services use).
- **Negative:** `opusenc` holds back the last frame until the next arrives, so the final 20 ms of each translated utterance publishes late and is usually dropped by viewers.

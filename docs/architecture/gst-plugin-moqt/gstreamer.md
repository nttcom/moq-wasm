# GStreamer Element Bindings for `moqtsink` (`gst-plugin-moqt`)

**Status:** Accepted (2026-09-18)

## What

Add `gstreamer` (`0.25`, already used by `crates/transcode`) to `gst-plugin-moqt`, a `cdylib` GStreamer plugin exposing the `moqtsink` element.

## Context

Operators repackaging SRT, RTMP, or file inputs into MoQT want to define pipelines in `gst-launch-1.0` (for example, `srtsrc ! tsdemux ! h264parse`) and let GStreamer handle demuxing and parsing. `crates/broadcast` already implements the MoQT session, MSF catalog, LOC/CMAF objects, and FETCH cache; the missing piece is a sink element that converts GStreamer buffers into `mediapack::MediaEvent`s.

## Alternatives

- **Add more input protocols to `services/ingest-gateway`:** Requires a custom listener and demuxer in Rust per protocol; GStreamer already supports them along with filters, encoders, and format conversion.
- **`appsink`-based standalone binary (`gstreamer-app`):** Fixes the pipeline in Rust and cannot be composed inside `gst-launch-1.0` or other GStreamer hosts.
- **Subclass `gst_base::BaseSink`:** Exposes only a single fixed sink pad, forcing audio and video onto two elements sharing one MoQT session out of band.

## Decision

Subclass `gst::Element` directly with two request sink pads (`video`, `audio`) so one element owns one MoQT session and demuxer pads link by caps. Declare the plugin descriptor with `gst::plugin_define!` using Cargo package metadata, avoiding a build script or `gst-plugin-version-helper`.

`gstreamer` is confined to `gst-plugin-moqt`, `crates/transcode`, `ingest-gateway`, `live-translate`, and `sip-gateway`; `moqt`, `mediapack`, and `broadcast` compile without it.

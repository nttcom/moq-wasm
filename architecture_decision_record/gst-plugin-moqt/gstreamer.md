# GStreamer element bindings for the MoQT sink

## Status

Accepted

## Date

2026-09-18

## What

Add the `gstreamer` crate (0.25, already used by `shared/transcode`) as the
dependency of the new `gst-plugin-moqt` crate, which is built as a `cdylib`
GStreamer plugin exposing the `moqtsink` element.

## Context

Operators re-packaging SRT, RTMP or file inputs into MoQT want to describe the
media path as a `gst-launch-1.0` pipeline and let GStreamer do demuxing and
parsing (`srtsrc ! tsdemux ! h264parse`). The publishing side — MoQT session,
MSF catalog, LOC/CMAF objects, FETCH cache — already exists in
`shared/media-publisher`; what is missing is a sink element that turns
GStreamer buffers into `mediapack::MediaEvent`s and hands them to it.

## Alternatives

- Extend `bridges/live-ingest` with more input protocols. Each protocol needs
  its own listener and demuxer in Rust; GStreamer already has them all and the
  operator can add filters, encoders and format conversion in the pipeline.
- An `appsink`-based program that links against `gstreamer-app` and owns the
  pipeline. Works, but the pipeline description is fixed in Rust and the
  element cannot be used from `gst-launch-1.0` or other GStreamer hosts.
- Deriving the element from `gst_base::BaseSink`. BaseSink has a single fixed
  sink pad, so audio and video would need two elements sharing one MoQT
  session through an out-of-band handle.

## Decision

Subclass `gst::Element` directly with two request sink pads (`video`, `audio`)
so one element owns one MoQT session and the pipeline can link demuxer pads to
it by caps. The plugin descriptor is declared with `gst::plugin_define!` using
Cargo package metadata, so no build script or `gst-plugin-version-helper`
dependency is needed. The `gstreamer` crate is confined to this crate and
`shared/transcode`; `moqt`, `mediapack` and `media-publisher` compile without
it.

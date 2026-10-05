# `moqt-bridge-live-ingest` Architecture

## Status
Living document. Update this file in the same change whenever the ingest flow
or the loss handling described here changes.

## Scope
`crates/moqt-bridge-live-ingest` accepts RTMP (FLV) and SRT (MPEG-TS), demuxes the input
with `mediapack`, optionally re-encodes it with `crates/transcode`, and hands
the resulting media events to `crates/media-publisher`, which owns the track
layout (see [its architecture document](../media-publisher/architecture.md)).

## Transport stream loss
The SRT listener reads with a 4 MiB UDP receive buffer: a keyframe arrives as
a burst of several hundred kilobytes within a few milliseconds, and the 64 KiB
that srt-tokio uses by default overflowed whenever the reader was not scheduled
at once, with the lost datagrams rarely recovered before their delivery time.

The MPEG-TS demuxer checks continuity counters. When packets are still lost,
the frame they cut is dropped along with the frames predicted from it until
the next keyframe, instead of being published corrupt for every viewer's
decoder to fail on. Each loss is logged as a warning, and the SRT statistics
are logged when a stream ends.

## Namespaces
The SRT stream id is the MoQT namespace: either the `r=` resource of an
access-control stream id (`#!::r=anon/live/test,m=publish`) or a plain path.
Connections without a stream id publish under `anon/srt/live`. RTMP uses the
application name of the URL. Relays only admit tokenless publishers into
`anon/**`, so both defaults stay inside it.

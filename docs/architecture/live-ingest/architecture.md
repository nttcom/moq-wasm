# `live-ingest` Architecture

## Status
Living document. Update this file in the same change whenever the ingest flow
or the loss handling described here changes.

## Scope
`crates/live-ingest` accepts RTMP (FLV), SRT (MPEG-TS) and plain RTP (H.264,
AAC, Opus and G.711 A-law described by an SDP file), demuxes the input with `mediapack`,
optionally re-encodes it with `crates/transcode`, and hands the resulting
media events to `crates/broadcast`, which owns the track layout (see
[its architecture document](../broadcast/architecture.md)).

Each input is one module (`rtmp`, `srt`, `rtp`) that owns its transport and
ends in the same `IngestPublisher`; the formats meet only at
`mediapack::MediaEvent`.

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

## RTP sessions
RTP has no connection, so the session is the SDP file given on the command
line: one UDP socket is bound per `m=` section, joining the group when the
`c=` address is multicast, and every socket feeds the one `rtp::Demuxer` of
the session, which tells the streams apart by payload type. The sockets take
the same 4 MiB receive buffer as SRT ([why socket2](socket2.md)). Parameter
sets announced only in the SDP seed the demuxer so the first keyframe is
decodable, as the ONVIF bridge found cameras require.

A stream starts with the first datagram and, since RTP carries no end
signal, ends after 5 seconds without one; the next datagram opens a new
`Broadcast` under the same namespace. The demuxer reorders packets per
stream and reports a gap once 200 ms of later packets have arrived behind
it; the frames the gap cuts are dropped until the next keyframe, as for
MPEG-TS, and each loss is logged as a warning.

## Namespaces
The SRT stream id is the MoQT namespace: either the `r=` resource of an
access-control stream id (`#!::r=anon/live/test,m=publish`) or a plain path.
Connections without a stream id publish under `anon/srt/live`. RTMP uses the
application name of the URL. RTP carries no name, so its namespace is the
`--rtp-namespace` argument, `anon/rtp/live` by default. Relays only admit
tokenless publishers into `anon/**`, so every default stays inside it.

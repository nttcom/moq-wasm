# `ingest-gateway` Architecture

## Status
Living document. Update this file in the same change whenever the ingest flow
or the loss handling described here changes.

## Scope
`services/ingest-gateway` bridges media from outside MoQT into a relay. The
subcommands split by which side opens the connection, not by protocol: the
protocols one side serves share their options and their deployment. Each
subcommand has a module of its own, and both hand
`mediapack::MediaEvent`s to `crates/broadcast`, which owns the track layout (see
[its architecture document](../broadcast/architecture.md)):

| Subcommand | Module | Shape |
| --- | --- | --- |
| `listen` | `listen` | A server: waits for RTMP, SRT and RTP pushes and opens one broadcast per stream |
| `pull` | `pull` | A client of one source: pulls an RTSP stream or an ONVIF camera's profiles into one broadcast |

`main` parses the command line, starts telemetry under the one service name
`ingest-gateway`, and runs the subcommand. The relay flags (`--moqt-url`,
`--auth-token`, `--insecure-skip-tls-verify`) are one `RelayArgs` both
subcommands flatten. The two modules share nothing else: a camera has to be
reachable from the process, so `pull` runs next to the camera, while `listen`
runs where publishers can reach its ports.

## listen
`listen` accepts RTMP (FLV), SRT (MPEG-TS) and plain RTP (H.264,
AAC, Opus and G.711 A-law described by an SDP file), demuxes the input with `mediapack`,
optionally re-encodes it with `crates/transcode`, and hands the resulting
media events to `crates/broadcast`, which owns the track layout (see
[its architecture document](../broadcast/architecture.md)).

Each input is one module (`rtmp`, `srt`, `rtp`) that owns its transport and
ends in the same `IngestPublisher`; the formats meet only at
`mediapack::MediaEvent`. An input runs only when its flag
(`--rtmp-addr`, `--srt-addr`, `--rtp-sdp`) is given.

Nothing on the input's task may block before the first sample is pushed:
`broadcast` takes the capture origin at that push, so a stall delays the origin
and stamps the backlog read after it into the future. The transcoder is
therefore built on a blocking thread, and the video samples that arrive
meanwhile wait in its input queue, which drops them once it is full.
`IngestPublisher` pushes each sample to the writer before the transcoder, so a
source keyframe has its group id by the time its renditions look it up (see
Group alignment in the `broadcast` document).

### Transport stream loss
The SRT listener reads with a 4 MiB UDP receive buffer: a keyframe arrives as
a burst of several hundred kilobytes within a few milliseconds, and the 64 KiB
that srt-tokio uses by default overflowed whenever the reader was not scheduled
at once, with the lost datagrams rarely recovered before their delivery time.

The MPEG-TS demuxer checks continuity counters. When packets are still lost,
the frame they cut is dropped along with the frames predicted from it until
the next keyframe, instead of being published corrupt for every viewer's
decoder to fail on. Each loss is logged as a warning, and the SRT statistics
are logged when a stream ends.

### RTP sessions
RTP has no connection, so the session is the SDP file given on the command
line: one UDP socket is bound per `m=` section, joining the group when the
`c=` address is multicast, and every socket feeds the one `rtp::Demuxer` of
the session, which tells the streams apart by payload type. The sockets take
the same 4 MiB receive buffer as SRT ([why socket2](socket2.md)). Parameter
sets announced only in the SDP seed the demuxer so the first keyframe is
decodable, as the ONVIF bridge found cameras require.

A stream starts with the first datagram and, since RTP carries no end
signal, ends after 5 seconds without one; the next datagram opens a new
`Broadcast` under the same namespace. A sender that restarts within those
5 seconds comes back with a new SSRC and new random sequence numbers and
timestamps, which cannot be ordered against the old ones, so the first
datagram from another SSRC than its stream began with also ends the stream
and opens the next `Broadcast` itself. The demuxer reorders packets per
stream and reports a gap once 200 ms of later packets have arrived behind
it; the frames the gap cuts are dropped until the next keyframe, as for
MPEG-TS, and each loss is logged as a warning.

### Namespaces
The SRT stream id is the MoQT namespace: either the `r=` resource of an
access-control stream id (`#!::r=anon/live/test,m=publish`) or a plain path.
Connections without a stream id publish under `anon/srt/live`. RTMP uses the
application name of the URL. RTP carries no name, so its namespace is the
`--rtp-namespace` argument, `anon/rtp/live` by default. Relays only admit
tokenless publishers into `anon/**`, so every default stays inside it.

## pull
One process serves one source: an RTSP stream (`--rtsp-url`), an ONVIF camera
(`--onvif-ip`), or both, when the stream replaces the camera's profiles and the
camera is only controlled. The `onvif` submodule holds everything ONVIF: it
reads the camera's media profiles (`GetProfiles`, `GetStreamUri`) and runs its
PTZ; the rest of `pull` only knows RTSP URLs.

`pull` declares a video and an audio track per profile in one `Broadcast`. Nothing is
pulled until the first SUBSCRIBE to the catalog or a profile track; then every
profile starts its RTSP session on GStreamer ([why GStreamer](gstreamer.md)),
and a session that ends is reconnected with a backoff of 1 to 30 seconds. Each
RTSP session gets a fresh `BroadcastWriter`, so a reconnected stream gets a new
capture origin for its restarted timestamps.

RTSP timestamps start when the camera starts sending, not when a sample
arrives, so `ArrivalClamp` shifts them until no capture time lies after its
sample's arrival ([proptest for its properties](proptest.md)).

With an ONVIF camera, `CommandSubscriber` follows the command track under
`--command-namespace` on the same session and forwards each command to the PTZ
worker, a thread that sends the ONVIF PTZ requests in order. The process ends
when the relay has ended every profile or the command subscription ends.

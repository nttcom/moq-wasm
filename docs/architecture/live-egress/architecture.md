# `live-egress` Architecture

## Status
Living document. Update this file in the same change whenever the reading
flow, the outputs or the invariants described here change.

## Scope
`crates/live-egress` is the reverse of `live-ingest`: it subscribes to an MSF
broadcast on a relay and hands its media to a protocol outside MoQT. Plain
RTP is the one output today. The broadcast is the one `crates/broadcast`
publishes, so the tracks are the ones its
[architecture document](../broadcast/architecture.md) describes.

## Layers
```
relay ──► BroadcastReader ──► mediapack::MediaEvent ──► output (rtp)
```

`BroadcastReader` knows MoQT and MSF and nothing of the outputs: it reads the
catalog, picks the tracks, and turns their LOC objects into media events on
one channel. An output knows its protocol and nothing of MoQT: it consumes the
media events. Further outputs, such as fMP4 or WebRTC, are further consumers
of the same channel; the packetization they need belongs in `mediapack`, as
`rtp::Muxer` does for RTP, so it stays free of sockets and reusable.

## Track selection
The reader takes the first LOC track with role `video` and the first with role
`audio` in catalog order, which is the source rendition in a `broadcast`
catalog. The audio track's AudioSpecificConfig comes from the `initDataList`
entry its `initRef` names (draft-ietf-moq-msf-01 §5.1.7, §5.2.13); an audio
track without one is skipped with a warning. CMAF tracks and the timeline are
ignored.

## Joining
The catalog is read with the joining FETCH beside its SUBSCRIBE, as MSF §5
asks of a late joiner, so the reader does not wait for the next catalog.
Media tracks are subscribed from the next group start rather than fetched:
FETCH would deliver past groups as a burst, which a real-time output like RTP
would send on as a burst. Every media track is subscribed before any is read,
so they start at the same group; `broadcast` starts audio groups at the video
keyframes with the same ids.

## Timing
Each track is demuxed by its own `loc::Demuxer`, measuring presentation time
from a capture origin the tracks share: the capture timestamp of the first
object any track delivers. LOC capture timestamps are on one wall clock across
the tracks of a broadcast, so the shared origin keeps video and audio aligned
in the media events, which an output that muxes both into one container
needs.

The RTP output carries this alignment only as far as RTP does: each stream has
its own random timestamp offset and no RTCP sender reports are sent, so a
receiver aligns the streams by when their first packets arrive.

## RTP output
The packets are sent from one unconnected UDP socket. A connected socket
reports a receiver that is not listening yet as an error on the next send,
which would end the output; RTP is fire-and-forget, so the receiver may start
at any time. The SDP is written once the muxer has the configuration of every
stream the reader announced, and names the parameter sets, so a receiver can
decode from the first keyframe it gets.

## End of stream
The output runs until every track has ended or the relay session closes.
A publisher that leaves resets the stream of the group in flight, which is
logged as a warning; the subscriptions stay open.

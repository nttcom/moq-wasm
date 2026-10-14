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
catalog. An AAC track's AudioSpecificConfig comes from the `initDataList`
entry its `initRef` names (draft-ietf-moq-msf-01 §5.1.7, §5.2.13); an AAC
track without one is skipped with a warning. A-law and Opus tracks carry no
initialization data, so their `codec`, `samplerate` and `channelConfig`
(§5.2.18, §5.2.28, §5.2.29) make up the configuration. CMAF tracks and the
timeline are ignored.

## Joining
The catalog is read with the joining FETCH beside its SUBSCRIBE, as MSF §5
asks of a late joiner, so the reader does not wait for the next catalog.
Media tracks are subscribed from the next group start rather than fetched:
FETCH would deliver past groups as a burst, which a real-time output like RTP
would send on as a burst. Every media track is subscribed before any is read,
which keeps their starts within one group of each other, but each still
starts at its own next group: when a SUBSCRIBE lands after a video keyframe
and before the first audio object of that group, the audio track starts one
group earlier. `broadcast` gives the audio groups the ids of the video groups
they start with, so the reader holds the audio to the group id the video
starts at and drops audio that arrives before the first video object, as
`broadcast` drops the audio before its first keyframe.

## Timing
Each track is demuxed by its own `loc::Demuxer`, measuring presentation time
from a capture origin the tracks share: the capture timestamp of the first
object any track delivers. LOC capture timestamps are on one wall clock across
the tracks of a broadcast, so the shared origin keeps video and audio aligned
in the media events, which an output that muxes both into one container
needs.

The RTP output hands this alignment to the receiver through RTCP sender
reports. Each stream keeps its own timestamp offset, as RTP requires. A
report pairs the time it is sent with the stream's RTP time at that instant
(RFC 3550 §6.4.1), extrapolated from the latest sample, whose wall-clock
time is the shared capture origin plus its presentation time; the same
capture timeline on every stream is what keeps them aligned. Both streams
carry one CNAME, so a receiver that reads the reports aligns them; one that
does not falls back to arrival times. A stream's first report goes out with
its first packet and the next every five seconds, the minimum interval RFC
3550 §6.2 recommends.

## RTP output
The packets are sent from one unconnected UDP socket. A connected socket
reports a receiver that is not listening yet as an error on the next send,
which would end the output; RTP is fire-and-forget, so the receiver may start
at any time. The SDP is written once the muxer has the configuration of every
stream the reader announced, and names the parameter sets, so a receiver can
decode from the first keyframe it gets. It also names each stream's SSRC with
the shared CNAME.

The payload types and SSRCs can be fixed on the command line, for a receiver
configured in advance rather than told by an SDP, as mediasoup's
PlainTransport is. Payload types are limited to the dynamic range, which also
keeps them out of the range RTCP multiplexing reserves (RFC 5761 §4). RTCP
goes to the port above each RTP port (RFC 3550 §11), or with `--rtcp-mux` to
the RTP port itself.

Keyframe requests (RTCP PLI / FIR) are not answered: the encoder sits behind
the relay, out of reach, so a receiver that starts late waits for the next
group the publisher opens.

## End of stream
The output runs until every track has ended or the relay session closes.
A publisher that leaves resets the stream of the group in flight, which is
logged as a warning; the subscriptions stay open.

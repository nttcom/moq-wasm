# `live-egress` Architecture

## Status
Living document. Update this file in the same change whenever the reading
flow, the outputs or the invariants described here change.

## Scope
`services/live-egress` is the reverse of `ingest-gateway`: it subscribes to an MSF
broadcast on a relay and hands its media to a protocol outside MoQT. Plain
RTP is the one output today. The broadcast is the one `crates/broadcast`
publishes, so the tracks are the ones its
[architecture document](../broadcast/architecture.md) describes.

## Layers
```
relay ──► BroadcastReader ──► mediapack::MediaEvent ──► output (rtp)
```

`broadcast::BroadcastReader` knows MoQT and MSF and nothing of the outputs: it
reads the catalog, picks the tracks, and turns their LOC objects into media
events on one channel. How it picks, joins and times the tracks is recorded in
the [`broadcast` architecture document](../broadcast/architecture.md) under
"Reading a broadcast". An output knows its protocol and nothing of MoQT: it
consumes the media events. Further outputs, such as fMP4 or WebRTC, are further consumers
of the same channel; the packetization they need belongs in `mediapack`, as
`rtp::Muxer` does for RTP, so it stays free of sockets and reusable.

## RTP timing
The reader times every track from one shared capture origin. The RTP output
hands this alignment to the receiver through RTCP sender reports. Each stream keeps its own timestamp offset, as RTP requires. A
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

# base64 for the SDP parameter sets of the RTP payload format

## Status

Accepted

## Date

2026-10-13

## What

Add `base64` as a dependency of the `mediapack` crate. `rtp::sdp` uses it
to read and write the `sprop-parameter-sets` attribute, the Base64-encoded
SPS and PPS an H.264 sender announces out of band (RFC 6184 §8.1).

## Context

`mediapack::rtp` carries H.264 and AAC over RTP. Every SDP an encoder writes
for a plain RTP session lists the parameter sets this way, and the LOC
publishers in this workspace expect them in band before each IDR, so the
parser has to decode the attribute to seed the parameter set tracker. The
muxer writes the same attribute so an SDP generated here plays in FFmpeg and
GStreamer.

## Alternatives

- Decode Base64 by hand. Some 40 lines, but a second implementation of a
  routine four other crates in the workspace already take from `base64`.
- Pass the attribute through as text and let the caller decode it. Moves the
  one place that needs the bytes out of the crate that defines the format.

## Decision

Depend on `base64`. `broadcast`, `auth-token`, `onvif-ingest` and `vts`
already use the same version, so no new crate enters the workspace lock.

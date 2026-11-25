# `base64` for RTP SDP Parameter Sets (`mediapack`)

**Status:** Accepted (2026-10-13)

## What

Add `base64` as a dependency of `mediapack`. `rtp::sdp` uses it to decode and encode `sprop-parameter-sets`, the Base64-encoded H.264 SPS/PPS announced out of band in SDP (RFC 6184 §8.1).

## Context

`mediapack::rtp` carries H.264 and AAC over RTP. Encoders advertise parameter sets via `sprop-parameter-sets`, while this workspace's LOC publishers require SPS/PPS in band before every IDR. The SDP parser must decode the attribute to seed `ParameterSetTracker`, and the muxer must encode it so generated SDPs play in FFmpeg and GStreamer.

## Alternatives

- **Hand-written Base64 decoder/encoder (~40 lines):** Duplicates functionality already provided by `base64` elsewhere in the workspace.
- **Pass the raw text attribute to callers:** Pushes format-specific decoding out of the crate that owns RTP/SDP parsing.

## Decision

Depend on `base64`. `broadcast`, `auth-token`, `ingest-gateway pull`, and `google-cloud` already use the same version, adding no new crate to the workspace lock.

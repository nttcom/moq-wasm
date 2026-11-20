# rustrtc for the phone leg's SDES-SRTP

## Status

Accepted

## Date

2026-11-20

## What

Add `rustrtc` 0.6 as a dependency of `sip-gateway`, with `std` and without
its default `srtp-ring` (AES-GCM then comes from RustCrypto's `aes-gcm`).
`media/rtp_leg.rs` builds one `PeerConnection` per call in its SIP mode
(`RtcConfiguration::sip()` with `Keying::Sdes`): it binds the media socket,
writes the SDP offer (`RTP/SAVP`, PCMU, PCMA, `telephone-event`, `a=crypto`),
applies the trunk's answer, protects the RTP the gateway sends and unprotects
what the trunk sends.

## Context

Secure Trunking sends SRTP keyed with SDES (RFC 4568) and refuses calls
without it. The gateway needs an SDP offer and answer for an `RTP/SAVP`
audio stream, SRTP with the suite the answer picks, and an RTP socket that
follows the trunk's address (symmetric RTP behind NAT). It does not need ICE
or DTLS.

## Alternatives

- `mediapack::rtp`. Already in the workspace and parses RTP and SDP, but has
  no SRTP, no `a=crypto` and no RFC 4733; all three would be written here.
- `webrtc-srtp` (webrtc-rs) with a hand-written SDP. Only the cipher; the
  offer, answer and key handling would still be the gateway's.
- `rvoip`'s media crates. Tied to its own session layer.

## Decision

Use `rustrtc`: it is the media stack written alongside `rsipstack`, its SIP
mode writes and reads SDES offers without ICE or DTLS attributes, and its
SRTP covers AES_CM_128_HMAC_SHA1_80 and _32 and AEAD_AES_128_GCM. ICE, DTLS
and SCTP still compile in (the crate has no feature to leave them out),
which adds about 50 crates to the lockfile but nothing to a call. G.711
transcoding and RFC 4733 events are not in it: transcoding uses GStreamer,
and the gateway sends its own RTP packets through
`PeerConnection::send_raw_rtp`.

Two of those crates carry advisories `cargo audit` reports: crypto-bigint
0.5 through `p256` (DTLS) and h2 0.3 through `igd` (UPnP port mapping).
Neither runs in the gateway, which keys SRTP with SDES and turns UPnP off, so
`.cargo/audit.toml` ignores both until rustrtc moves to newer releases.

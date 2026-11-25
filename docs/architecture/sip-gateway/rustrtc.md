# `rustrtc` for SDES-SRTP Media (`sip-gateway`)

**Status:** Accepted (2026-11-20)

## What

Add `rustrtc` 0.6 (`default-features = false`, feature: `std`; omits `srtp-ring` so AES-GCM uses RustCrypto's `aes-gcm`) to `sip-gateway`. `media/rtp_leg.rs` creates one `PeerConnection` per call in SIP mode (`RtcConfiguration::sip()`, `Keying::Sdes`) to bind the media socket, write the SDP offer (`RTP/SAVP`, PCMU, PCMA, `telephone-event`, `a=crypto`), apply the trunk's answer, and protect/unprotect SRTP packets.

## Context

Secure Trunking requires SDES-keyed SRTP (RFC 4568) and rejects unencrypted calls. The gateway needs `RTP/SAVP` SDP offer/answer negotiation, SRTP for the negotiated cipher suite, and symmetric RTP (following the trunk's address behind NAT), without ICE or DTLS.

## Alternatives

- **`mediapack::rtp`:** Already in the workspace for plain RTP/SDP, but lacks SRTP, `a=crypto`, and RFC 4733 DTMF.
- **`webrtc-srtp` (`webrtc-rs`) + manual SDP:** Provides only the cipher, leaving SDP offer/answer and key management to hand-written code.
- **`rvoip` media crates:** Tightly coupled to `rvoip`'s session layer.

## Decision

Use `rustrtc`: built alongside `rsipstack`, its SIP mode generates/parses SDES offers without ICE/DTLS attributes and supports `AES_CM_128_HMAC_SHA1_80`, `AES_CM_128_HMAC_SHA1_32`, and `AEAD_AES_128_GCM`. Transcoding uses GStreamer, and RFC 4733 DTMF packets are sent via `PeerConnection::send_raw_rtp`.

## Consequences

- ICE, DTLS, and SCTP still compile in (no feature flags exclude them), adding ~50 crates to `Cargo.lock` without running during calls.
- Two transitive crates trigger `cargo audit` advisories: `crypto-bigint` 0.5 (via `p256`/DTLS) and `h2` 0.3 (via `igd`/UPnP). Neither code path executes (SDES keying, UPnP disabled), so `.cargo/audit.toml` ignores both until `rustrtc` updates.

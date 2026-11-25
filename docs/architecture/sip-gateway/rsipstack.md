# `rsipstack` for SIP Signalling (`sip-gateway`)

**Status:** Accepted (2026-11-20)

## What

Add `rsipstack` 0.8 (`default-features = false`, features: `platform-tokio`, `rustls-aws-lc-rs`; omits `rustls-ring`, WebSocket, and SRV lookup) to `sip-gateway`. `sip_trunk.rs` runs one endpoint with a TLS listener and drives calls through `DialogLayer::do_invite_async` (handling 401/407 digest auth, dialog state transitions, and `BYE`/`CANCEL`).

## Context

`sip-gateway` speaks SIP over TLS to SIP trunks (Twilio Elastic SIP Trunking, plus local Asterisk for development): `INVITE` with SDP offer, digest authentication, provisional/final responses, `ACK`, bidirectional `BYE`, `CANCEL` while ringing, and inbound `INVITE`s. TLS is mandatory because SRTP keys travel inside SDP.

## Alternatives

- **`rvoip`:** Monolithic softphone stack (SIP, media, session), still alpha with large, irregular releases.
- **`ezk-sip`:** Small transaction/dialog crate, rarely released and lightly battle-tested against real trunks.
- **`rsip` alone:** Message parser only (no transactions/dialogs) and unmaintained since 2022.
- **Hand-written transactions and dialogs:** Reimplements RFC 3261 timers, retransmissions, forked responses, and `CANCEL` race handling.

## Decision

Use `rsipstack`: RFC 3261/3262 transactions and dialogs, digest auth, `rustls` TLS (sharing one `aws-lc-rs` provider), frequent releases, and production use in the authors' PBX. Media is handled by `rustrtc`, and RFC 4028 session timers are omitted or handled by the gateway.

> [!WARNING]
> `rsipstack` logs full SIP messages—including SDP crypto keys and digest responses—at `INFO`. The gateway's default log filter clamps `rsipstack` to `WARN`.

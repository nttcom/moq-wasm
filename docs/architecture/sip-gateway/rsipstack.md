# rsipstack for the gateway's SIP signalling

## Status

Accepted

## Date

2026-11-20

## What

Add `rsipstack` 0.8 as a dependency of `sip-gateway`, built with
`platform-tokio` and `rustls-aws-lc-rs` and without its default features
(`rustls-ring`, WebSocket, SRV lookup). `sip_trunk.rs` runs one endpoint with
a TLS listener and sends each call's INVITE through its dialog layer
(`DialogLayer::do_invite_async`), which answers a 401/407 with the trunk's
credentials, follows the dialog's states and sends BYE or CANCEL.

## Context

The gateway speaks SIP to a trunk (Twilio Elastic SIP Trunking, and a local
Asterisk until there is an account): INVITE with an SDP offer, digest
authentication on INVITE, provisional and final responses, ACK, BYE in both
directions, CANCEL while ringing, and later answering INVITEs. Signalling must
be TLS only, because the SRTP keys travel in the SDP.

## Alternatives

- `rvoip`. A whole softphone stack (SIP, media, session) in one workspace,
  still alpha and released in large, irregular steps.
- `ezk-sip`. Transactions and dialogs in a small crate, but rarely released
  and with few users to have met trunks' quirks.
- `rsip` alone. A message parser with no transactions or dialogs, and
  unmaintained since 2022.
- Writing transactions and dialogs on top of a parser. RFC 3261 timers,
  retransmissions, forked responses and CANCEL races are what a stack is for.

## Decision

Use `rsipstack`: transactions and dialogs to RFC 3261/3262, digest
authentication, TLS through rustls (so the process keeps one aws-lc-rs
provider), released every few weeks, and used in production by the same
authors' PBX. It has no media and no session timers (RFC 4028), which the
gateway brings (`rustrtc`) or adds itself.

It logs every SIP message whole at INFO, SDP crypto keys and digest
responses included, so the gateway's default log directive keeps it at WARN.

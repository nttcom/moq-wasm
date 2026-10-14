# socket2 for the UDP receive buffer of the RTP input

## Status

Accepted

## Date

2026-10-13

## What

Add `socket2` as a dependency of `live-ingest`. The RTP listener builds its
UDP sockets with it to set a 4 MiB receive buffer and join a multicast group
before handing the socket to tokio.

## Context

A keyframe arrives as a burst of several hundred kilobytes within a few
milliseconds. The SRT input hit this with srt-tokio's 64 KiB default buffer:
whenever the reader was not scheduled at once the kernel dropped datagrams,
and the losses were rarely recovered before their delivery time (see
[architecture](architecture.md)). RTP has no retransmission at all, so a
dropped datagram is a lost frame. srt-tokio exposes the buffer size as an
option; `tokio::net::UdpSocket` does not, and neither does `std::net::UdpSocket`.

## Alternatives

- Call `setsockopt` through `libc`. Works, but is `unsafe` for a one-line
  option and needs a Windows branch.
- Rely on the kernel default and raise it system-wide (`net.core.rmem_max`).
  Moves a requirement of the bridge into deployment instructions.
- Keep draining the socket from a dedicated thread. Does not help: the
  datagrams are dropped by the kernel before any reader sees them.

## Decision

Depend on `socket2`, the crate tokio and quinn themselves use for socket
options; it is already in the workspace lock, so no new crate enters the
dependency tree.

# `socket2` for RTP UDP Receive Buffers (`ingest-gateway`)

**Status:** Accepted (2026-10-13)

## What

Add `socket2` as a dependency of `ingest-gateway`. The RTP listener uses it to configure a 4 MiB UDP receive buffer and join multicast groups before converting the socket to Tokio.

## Context

Video keyframes arrive as bursts of several hundred kilobytes within milliseconds. The SRT input suffered kernel datagram drops with `srt-tokio`'s 64 KiB default buffer whenever the reader was briefly unscheduled (see [architecture.md](architecture.md)). RTP has no retransmission, so every dropped datagram loses a frame. Unlike `srt-tokio`, neither `tokio::net::UdpSocket` nor `std::net::UdpSocket` exposes receive-buffer sizing.

## Alternatives

- **`setsockopt` via `libc`:** Requires `unsafe` and a separate Windows branch for a single socket option.
- **System-wide kernel tuning (`net.core.rmem_max`):** Pushes an application requirement into deployment configuration.
- **Dedicated socket-draining thread:** Does not prevent drops that happen in the kernel before user space reads.

## Decision

Depend on `socket2`—the same crate `tokio` and `quinn` use for socket options. It is already in the workspace lock, adding no new crate to the dependency tree.

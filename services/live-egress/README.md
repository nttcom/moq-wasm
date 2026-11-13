# live-egress

Subscribes to a broadcast on a MoQT relay and sends it as plain RTP: H.264 on
one port, AAC, Opus or G.711 A-law on another, RTCP sender reports beside
them, and an SDP file for the receiver. The broadcast is the one described in
[`crates/broadcast`](../../crates/broadcast/README.md).
Design: [architecture](../../docs/architecture/live-egress/architecture.md).

## Run

```shell
make relay
make live-ingest
make ffmpeg-srt
make live-egress
ffplay -protocol_whitelist file,udp,rtp assets/egress.sdp
```

| Variable | Default |
| --- | --- |
| `LIVE_EGRESS_NAMESPACE` | `anon/live/test` |
| `LIVE_EGRESS_DESTINATION` | `127.0.0.1` |
| `LIVE_EGRESS_VIDEO_PORT` / `LIVE_EGRESS_AUDIO_PORT` | `5014` / `5016` |
| `LIVE_EGRESS_SDP` | `assets/egress.sdp` |
| `LIVE_EGRESS_MOQT_URL` | the local relay |

The first LOC video track and the first LOC audio track of the catalog are
sent, starting at the next group.

## CLI

```shell
cargo run -p live-egress -- \
  --moqt-url https://127.0.0.1:4433 \
  --namespace anon/live/test \
  [--destination 127.0.0.1] [--video-port 5004] [--audio-port 5006] \
  [--video-payload-type 96] [--audio-payload-type 97] \
  [--video-ssrc <n>] [--audio-ssrc <n>] [--rtcp-mux] \
  [--sdp-out egress.sdp] [--insecure-skip-tls-verify] [--auth-token <jwt>]
```

| Option | Effect |
| --- | --- |
| `--video-payload-type` / `--audio-payload-type` | Payload type of each stream, 96–127 |
| `--video-ssrc` / `--audio-ssrc` | SSRC of each stream; random when absent |
| `--rtcp-mux` | Send RTCP on each stream's RTP port instead of the port above it |
| `--insecure-skip-tls-verify` | Skip TLS certificate verification, for a local relay with a self-signed certificate only |

A receiver configured in advance, such as a mediasoup PlainTransport, takes
the same payload types, SSRCs and RTCP port as these options.

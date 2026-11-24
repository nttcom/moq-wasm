# egress-gateway

Subscribes to an MSF broadcast ([`crates/broadcast`](../../crates/broadcast/README.md)) on a MoQT relay and outputs plain RTP (H.264 video, AAC/Opus/G.711 A-law audio, RTCP sender reports, and an SDP file). Design: [architecture.md](../../docs/architecture/egress-gateway/architecture.md).

## Run

Run each command in its own terminal from the repository root:

```shell
make relay
make ingest-gateway
make ffmpeg-srt
make egress-gateway
```

Once `assets/egress.sdp` is written, play the stream:

```shell
ffplay -protocol_whitelist file,udp,rtp assets/egress.sdp
```

| Variable | Default |
| --- | --- |
| `EGRESS_GATEWAY_NAMESPACE` | `anon/live/test` |
| `EGRESS_GATEWAY_DESTINATION` | `127.0.0.1` |
| `EGRESS_GATEWAY_VIDEO_PORT` / `EGRESS_GATEWAY_AUDIO_PORT` | `5014` / `5016` |
| `EGRESS_GATEWAY_SDP` | `assets/egress.sdp` |
| `EGRESS_GATEWAY_MOQT_URL` | local relay |

Sends the tallest LOC video track and the highest-bitrate LOC audio track of the catalog's main source, starting at the next group. For all CLI flags (payload types, SSRCs, RTCP muxing), run `cargo run -p egress-gateway -- --help`.

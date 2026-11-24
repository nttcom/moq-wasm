# egress-gateway

Subscribes to a broadcast on a MoQT relay and sends it as plain RTP: H.264 on
one port, AAC, Opus or G.711 A-law on another, RTCP sender reports beside
them, and an SDP file for the receiver. The broadcast is the one described in
[`crates/broadcast`](../../crates/broadcast/README.md).
Design: [architecture](../../docs/architecture/egress-gateway/architecture.md).

## Run

Run each command in a separate terminal from the workspace root:

```shell
make relay
make ingest-gateway
make ffmpeg-srt
make egress-gateway
```

Once the bridge writes `assets/egress.sdp`, play the test pattern and tone:

```shell
ffplay -protocol_whitelist file,udp,rtp assets/egress.sdp
```

| Variable | Default |
| --- | --- |
| `EGRESS_GATEWAY_NAMESPACE` | `anon/live/test` |
| `EGRESS_GATEWAY_DESTINATION` | `127.0.0.1` |
| `EGRESS_GATEWAY_VIDEO_PORT` / `EGRESS_GATEWAY_AUDIO_PORT` | `5014` / `5016` |
| `EGRESS_GATEWAY_SDP` | `assets/egress.sdp` |
| `EGRESS_GATEWAY_MOQT_URL` | the local relay |

The first LOC video track and the first LOC audio track of the catalog are
sent, starting at the next group.

For all CLI options, including RTP payload types, SSRCs and RTCP multiplexing
for a preconfigured receiver, run `cargo run -p egress-gateway -- --help`.

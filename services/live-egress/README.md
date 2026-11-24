# live-egress

Subscribes to a broadcast on a MoQT relay and sends it as plain RTP: H.264 on
one port, AAC, Opus or G.711 A-law on another, RTCP sender reports beside
them, and an SDP file for the receiver. The broadcast is the one described in
[`crates/broadcast`](../../crates/broadcast/README.md).
Design: [architecture](../../docs/architecture/live-egress/architecture.md).

## Run

Run each command in a separate terminal from the workspace root:

```shell
make relay
make ingest-gateway
make ffmpeg-srt
make live-egress
```

Once the bridge writes `assets/egress.sdp`, play the test pattern and tone:

```shell
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

For all CLI options, including RTP payload types, SSRCs and RTCP multiplexing
for a preconfigured receiver, run `cargo run -p live-egress -- --help`.

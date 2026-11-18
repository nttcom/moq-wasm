# live-ingest

Publishes RTMP, SRT or RTP input into a MoQT relay as the tracks described in
[`crates/broadcast`](../../crates/broadcast/README.md).
Design: [architecture](../../docs/architecture/live-ingest/architecture.md).

## Run

Run each command in a separate terminal from the workspace root:

```shell
make relay
make live-ingest
make ffmpeg-srt
```

In [Live Viewer](../../examples/browser/examples/live-viewer/README.md), select the local relay,
enter `anon/live/test` and press **Watch** to see the test pattern.

| Input | Listens on | Namespace |
| --- | --- | --- |
| RTMP | `0.0.0.0:1935` | Application name of the URL: `rtmp://host:1935/anon/live/test/<key>` |
| SRT | `0.0.0.0:9000` | Stream id: `anon/live/test` or `#!::r=anon/live/test,m=publish`; `anon/srt/live` when absent |
| RTP | The ports of the SDP file (`make live-ingest-rtp`) | `--rtp-namespace`, `anon/rtp/live` by default |

Tokenless publishers may only use `anon/**`.

| Variant | Effect |
| --- | --- |
| `LIVE_INGEST_MOQT_URL=https://relay.example.com:443 make live-ingest` | Publish to another relay |
| `make live-ingest-transcode` | Add `video_720p` / `video_480p` / `video_360p` renditions. Needs GStreamer, see [`crates/transcode`](../../crates/transcode/README.md) |
| `make live-ingest-stats` | Print the QUIC statistics of every relay connection once a second |
| `make live-ingest-rtp` | Also receive the RTP session described by `LIVE_INGEST_RTP_SDP` (default `assets/rtp.sdp`) |

## Send a test stream

These sources publish to `anon/live/test`.

| Command | Source |
| --- | --- |
| `make ffmpeg-rtmp` | Test pattern and 1 kHz tone over RTMP |
| `make ffmpeg-rtp` | Test pattern and tone over RTP; writes `assets/rtp.sdp`, so start it before `make live-ingest-rtp` |
| `make ffmpeg-srt-bbb-local` | Big Buck Bunny, looped, to `localhost:9000` |
| `make ffmpeg-srt-bbb-remote` | Big Buck Bunny to `relay-1.moqt.research.skyway.io:9000`; no local bridge or relay needed |

The Big Buck Bunny commands download the film (263 MiB) into `assets/bbb/` on
first use. Big Buck Bunny is (c) 2008 Blender Foundation | www.bigbuckbunny.org,
[CC BY 3.0](https://creativecommons.org/licenses/by/3.0/).

## Behaviour

- When MPEG-TS or RTP packets are lost, frames are dropped until the next
  keyframe and each loss is logged as a warning.
- An RTP stream ends after 5 seconds without a datagram, or when its sender
  restarts with a new SSRC.
- `RUST_LOG=info,broadcast::delivery=debug make live-ingest` logs one line per
  sample as it enters the publisher and as it is sent to the relay; the Live
  Viewer's delivery check reads this log.

For all CLI options, run `cargo run -p live-ingest -- --help`.

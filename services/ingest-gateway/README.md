# ingest-gateway

Bridges media from outside MoQT into a relay as the MSF broadcasts described in
[`crates/broadcast`](../../crates/broadcast/README.md). Each source kind is a
subcommand:

| Subcommand | Source |
| --- | --- |
| `listen` | Listens for RTMP, SRT and RTP pushes, one broadcast per stream |
| `onvif` | Pulls an ONVIF camera's RTSP profiles and applies the PTZ commands it subscribes to |

`--moqt-url`, `--auth-token` (or `MOQT_AUTH_TOKEN`) and
`--insecure-skip-tls-verify` select the relay for both.
Design: [architecture](../../docs/architecture/ingest-gateway/architecture.md).

## listen

### Run

Run each command in a separate terminal from the workspace root:

```shell
make relay
make ingest-gateway
make ffmpeg-srt
```

In [Live Viewer](../../examples/browser/examples/live-viewer/README.md), select the local relay,
enter `anon/live/test` and press **Watch** to see the test pattern.

| Input | Listens on | Namespace |
| --- | --- | --- |
| RTMP | `0.0.0.0:1935` | Application name of the URL: `rtmp://host:1935/anon/live/test/<key>` |
| SRT | `0.0.0.0:9000` | Stream id: `anon/live/test` or `#!::r=anon/live/test,m=publish`; `anon/srt/live` when absent |
| RTP | The ports of the SDP file (`make ingest-gateway-rtp`) | `--rtp-namespace`, `anon/rtp/live` by default |

Tokenless publishers may only use `anon/**`.

| Variant | Effect |
| --- | --- |
| `INGEST_GATEWAY_MOQT_URL=https://relay.example.com:443 make ingest-gateway` | Publish to another relay |
| `make ingest-gateway-transcode` | Add `video_720p` / `video_480p` / `video_360p` renditions. Needs GStreamer, see [`crates/transcode`](../../crates/transcode/README.md) |
| `make ingest-gateway-stats` | Print the QUIC statistics of every relay connection once a second |
| `make ingest-gateway-rtp` | Also receive the RTP session described by `INGEST_GATEWAY_RTP_SDP` (default `assets/rtp.sdp`) |

### Send a test stream

These sources publish to `anon/live/test`.

| Command | Source |
| --- | --- |
| `make ffmpeg-rtmp` | Test pattern and 1 kHz tone over RTMP |
| `make ffmpeg-rtp` | Test pattern and tone over RTP; writes `assets/rtp.sdp`, so start it before `make ingest-gateway-rtp` |
| `make ffmpeg-srt-bbb-local` | Big Buck Bunny, looped, to `localhost:9000` |
| `make ffmpeg-srt-bbb-remote` | Big Buck Bunny to `relay-1.moqt.research.skyway.io:9000`; no local bridge or relay needed |

The Big Buck Bunny commands download the film (263 MiB) into `assets/bbb/` on
first use. Big Buck Bunny is (c) 2008 Blender Foundation | www.bigbuckbunny.org,
[CC BY 3.0](https://creativecommons.org/licenses/by/3.0/).

### Behaviour

- When MPEG-TS or RTP packets are lost, frames are dropped until the next
  keyframe and each loss is logged as a warning.
- An RTP stream ends after 5 seconds without a datagram, or when its sender
  restarts with a new SSRC.
- `RUST_LOG=info,broadcast::delivery=debug make ingest-gateway` logs one line per
  sample as it enters the publisher and as it is sent to the relay; the Live
  Viewer's delivery check reads this log.

For all CLI options, run `cargo run -p ingest-gateway -- listen --help`.

## onvif

### Setup

The RTSP input runs on GStreamer with the good and bad plugins:
`brew install gstreamer`, or on Debian/Ubuntu `libgstreamer1.0-dev`,
`gstreamer1.0-plugins-good` and `gstreamer1.0-plugins-bad`.

```shell
cp .env.example .env    # set ONVIF_IP, ONVIF_USERNAME, ONVIF_PASSWORD and MOQT_URL
make onvif
```

Every camera profile starts its RTSP stream when the first viewer subscribes
to the catalog or to one of its tracks. `ONVIF_MOQT_URL` overrides the relay
URL `make onvif` resolves.

### Tracks

The tracks form one MSF broadcast built by
[`crates/broadcast`](../../crates/broadcast/README.md).

| Role | Namespace | Track name |
| --- | --- | --- |
| Catalog | `anon/onvif/client` | `catalog` |
| Video (profile N) | `anon/onvif/client` | `video/profile_N` (LOC), `video/profile_N_cmaf` (CMAF) |
| Audio (profile N) | `anon/onvif/client` | `audio/profile_N` (LOC; G.711 A-law, AAC or Opus), `audio/profile_N_cmaf` (CMAF, AAC only) |
| PTZ command (subscribed) | `anon/onvif/viewer` | `command`, one JSON object per subgroup stream |

Profile indices start at 1 and follow the ONVIF media profiles the camera
returns. `--publish-namespace`, `--subscribe-namespace`, `--video-track`,
`--audio-track` and `--command-track` change the names.

### Run without make

```shell
cargo run -p ingest-gateway -- onvif \
  --ip 192.168.11.45 \
  --username admin \
  --password secret \
  --moqt-url https://127.0.0.1:4433 \
  --insecure-skip-tls-verify
```

| Option | Meaning |
| --- | --- |
| `--ip`, `--username`, `--password` | Camera address and ONVIF credentials |
| `--moqt-url` | MoQT relay URL |
| `--rtsp-url` | Publish this RTSP stream as profile 1 instead of the camera's ONVIF profiles |
| `--insecure-skip-tls-verify` | Skip TLS certificate verification, for local development only |
| `--dump-keyframe[=PATH]` | Dump the first keyframe payload of profile 1 for ffprobe |

# ingest-gateway

Bridges external media into a MoQT relay as MSF broadcasts ([`crates/broadcast`](../../crates/broadcast/README.md)). Design: [architecture.md](../../docs/architecture/ingest-gateway/architecture.md).

| Subcommand | Source |
| --- | --- |
| `listen` | Listens for RTMP, SRT, and RTP pushes (one broadcast per stream) |
| `pull` | Pulls an RTSP stream or an ONVIF camera's profiles (and executes subscribed PTZ commands for ONVIF) |

Both subcommands share `--moqt-url`, `--auth-token` (`MOQT_AUTH_TOKEN`), and `--insecure-skip-tls-verify`.

## `listen`

Run each command in its own terminal from the repository root, then watch `anon/live/test` in [Live Viewer](../../examples/browser/examples/live-viewer/README.md):

```shell
make relay
make ingest-gateway
make ffmpeg-srt
```

Each input is enabled only when its flag is passed (at least one required); `make ingest-gateway` enables RTMP (`0.0.0.0:1935`) and SRT (`0.0.0.0:9000`).

| Input | Flag | Namespace |
| --- | --- | --- |
| RTMP | `--rtmp-addr` | URL app name: `rtmp://host:1935/anon/live/test/<key>` |
| SRT | `--srt-addr` | Stream ID (`anon/live/test` or `#!::r=anon/live/test,m=publish`); default `anon/srt/live` |
| RTP | `--rtp-sdp` (`make ingest-gateway-rtp`) | `--rtp-namespace` (CLI default `anon/rtp/live`; `make ingest-gateway-rtp` passes `anon/live/test`) |

Publishers without a token can only use `anon/**`.

| Variant | Effect |
| --- | --- |
| `INGEST_GATEWAY_MOQT_URL=https://relay.example.com:443 make ingest-gateway` | Target another relay |
| `make ingest-gateway-transcode` | Add `video_720p`/`480p`/`360p` renditions (requires GStreamer; see [`crates/transcode`](../../crates/transcode/README.md)) |
| `make ingest-gateway-stats` | Print QUIC stats every second |
| `make ingest-gateway-rtp` | Also ingest the RTP SDP at `INGEST_GATEWAY_RTP_SDP` (default `assets/rtp.sdp`) |

### Test streams

All commands publish to `anon/live/test`:

| Command | Source |
| --- | --- |
| `make ffmpeg-rtmp` | Test pattern + 1 kHz tone over RTMP |
| `make ffmpeg-rtp` | Test pattern + tone over RTP (writes `assets/rtp.sdp`; start before `make ingest-gateway-rtp`) |
| `make ffmpeg-srt-bbb-local` | Looped Big Buck Bunny to `localhost:9000` |
| `make ffmpeg-srt-bbb-remote` | Big Buck Bunny to `relay-1.moqt.research.skyway.io:9000` (no local relay needed) |

Big Buck Bunny (263 MiB) downloads to `assets/bbb/` on first use ((c) 2008 Blender Foundation | www.bigbuckbunny.org, [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/)).

### Behaviour

- On MPEG-TS or RTP packet loss, frames drop until the next keyframe (logged at `WARN`).
- An RTP stream ends after 5 s without a datagram or on SSRC change.
- `RUST_LOG=info,broadcast::delivery=debug make ingest-gateway` logs one line per sample as it enters the publisher and as it is sent to the relay, which Live Viewer's delivery check compares.

For all CLI flags, run `cargo run -p ingest-gateway -- listen --help`.

## `pull`

Pulls a direct RTSP URL (`--rtsp-url`) or an ONVIF camera's profiles (`--onvif-ip`, enabling PTZ control). At least one is required; passing both publishes `--rtsp-url` as profile 1 while keeping ONVIF PTZ active. Run on a network that can reach the camera.

### Setup

Requires GStreamer with `-good` and `-bad` plugins (`brew install gstreamer` on macOS; `pkg-config`, `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, `gstreamer1.0-plugins-good`, `gstreamer1.0-plugins-bad` on Debian/Ubuntu):

```shell
cp .env.example .env    # set ONVIF_IP, ONVIF_USERNAME, ONVIF_PASSWORD, MOQT_URL
make onvif
```

Each profile starts its RTSP pull lazily when the first viewer subscribes to `catalog` or a track. Override the relay URL with `ONVIF_MOQT_URL`.

### Tracks

Published as one MSF broadcast ([`crates/broadcast`](../../crates/broadcast/README.md)):

| Role | Namespace | Track |
| --- | --- | --- |
| Catalog | `anon/onvif/client` | `catalog` |
| Video (profile N) | `anon/onvif/client` | `video/profile_N` (LOC), `video/profile_N_cmaf` (CMAF) |
| Audio (profile N) | `anon/onvif/client` | `audio/profile_N` (LOC: G.711 A-law, AAC, Opus), `audio/profile_N_cmaf` (CMAF: AAC only) |
| PTZ commands (subscribed, `--onvif-ip` only) | `anon/onvif/viewer` | `command` (one JSON object per subgroup stream) |

Profile indices start at `1` in ONVIF profile order (`--rtsp-url` is profile 1). Customize names with `--publish-namespace`, `--command-namespace`, `--video-track`, `--audio-track`, and `--command-track`.

### CLI

```shell
# Direct RTSP stream (no PTZ)
cargo run -p ingest-gateway -- pull \
  --rtsp-url rtsp://user:secret@192.168.11.45:554/stream1 \
  --moqt-url https://127.0.0.1:4433 --insecure-skip-tls-verify

# ONVIF camera (media profiles + PTZ)
cargo run -p ingest-gateway -- pull \
  --onvif-ip 192.168.11.45 --onvif-username admin --onvif-password secret \
  --moqt-url https://127.0.0.1:4433 --insecure-skip-tls-verify
```

| Option | Meaning |
| --- | --- |
| `--rtsp-url` | Publish a direct RTSP URL as profile 1 instead of querying ONVIF profiles |
| `--onvif-ip`, `--onvif-username`, `--onvif-password` | ONVIF camera address and credentials (enables PTZ) |
| `--moqt-url` | MoQT relay URL |
| `--insecure-skip-tls-verify` | Skip TLS verification (local dev only) |
| `--dump-keyframe[=PATH]` | Dump profile 1's first keyframe payload for `ffprobe` |

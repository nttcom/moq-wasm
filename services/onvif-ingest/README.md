# onvif-ingest

ONVIF/RTSP bridge that publishes an ONVIF camera's media over MoQT and
applies the PTZ commands it subscribes to.

## Setup

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

## Tracks

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

## CLI

```shell
cargo run -p onvif-ingest -- \
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

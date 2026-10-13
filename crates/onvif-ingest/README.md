# onvif-ingest

ONVIF/RTSP bridge for controlling an ONVIF camera and publishing its media over MoQT.

## Setup

The RTSP input runs on GStreamer, so install it with the good and bad plugins
(`rtspsrc`, the RTP depayloaders and `h264parse`):

```shell
brew install gstreamer
```

On Debian/Ubuntu, install `libgstreamer1.0-dev`, `gstreamer1.0-plugins-good`
and `gstreamer1.0-plugins-bad`.

Create `.env` from `.env.example` and set:

- `ONVIF_IP`
- `ONVIF_USERNAME`
- `ONVIF_PASSWORD`
- `MOQT_URL` when publishing to a MoQT relay

```shell
cp .env.example .env
```

## Run

```shell
make onvif
```

`make onvif` publishes RTSP video/audio as profile-specific MoQT tracks and subscribes to ONVIF commands, one JSON object per subgroup stream.
The tracks form one MSF broadcast built by [`crates/broadcast`](../broadcast/README.md). Every profile starts its RTSP stream when the first viewer subscribes to the catalog or to one of the profile tracks, and keeps running until that stream ends; the bridge exits once every profile has ended.

When `MOQT_URL` points to `localhost` or `127.0.0.1` and the docker compose `relay-a`
service is running, `make onvif` rewrites the relay URL to the Docker Desktop bridge
host used by native Rust clients. Override the resolved URL with `ONVIF_MOQT_URL`, or
set `MOQT_DOCKER_RELAY_HOST`/`LOCAL_RELAY_HOST` to force a specific relay host.

## MoQT Track Layout

`make onvif` publishes to namespace `anon/onvif/client` and subscribes to commands from namespace `anon/onvif/viewer`.

| Role                    | Namespace      | Track name        |
| ----------------------- | -------------- | ----------------- |
| Catalog                 | `anon/onvif/client` | `catalog`         |
| Video (profile N)       | `anon/onvif/client` | `video/profile_N` (LOC), `video/profile_N_cmaf` (CMAF) |
| Audio (profile N)       | `anon/onvif/client` | `audio/profile_N` (LOC; G.711 A-law, AAC or Opus), `audio/profile_N_cmaf` (CMAF, AAC only) |
| PTZ command (subscribe) | `anon/onvif/viewer` | `command`         |

Profile indices start at 1 and correspond to the ONVIF media profiles returned by the camera.
Video objects are Annex-B, and each group starts with a keyframe that carries SPS/PPS. Audio groups start with the video keyframe groups.
The names are configurable via CLI flags (`--publish-namespace`, `--subscribe-namespace`, `--video-track`, `--audio-track`, `--command-track`); the catalog track is always `catalog`.

## Direct CLI Options

Use `cargo run` directly when you need options that are not exposed by the Makefile helpers.

```shell
cargo run -p onvif-ingest -- \
  --ip 192.168.11.45 \
  --username admin \
  --password secret \
  --moqt-url https://127.0.0.1:4433 \
  --dump-keyframe \
  --insecure-skip-tls-verify
```

Common options:

- `--ip`: camera IP address or hostname
- `--username`, `--password`: ONVIF credentials
- `--moqt-url`: MoQT relay URL
- `--rtsp-url`: publish this RTSP stream as profile 1 instead of the camera's ONVIF profiles, e.g. a local test server (`--ip` and the credentials are still used for PTZ)
- `--insecure-skip-tls-verify`: skip TLS certificate verification for local development
  only
- `--dump-keyframe[=PATH]`: dump the first keyframe payload of profile 1 for ffprobe

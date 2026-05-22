# moqt-bridge-onvif

ONVIF/RTSP bridge for controlling an ONVIF camera and publishing its media over MoQT.

## Setup

Create `.env` from `.env.example` and set:

- `ONVIF_IP`
- `ONVIF_USERNAME`
- `ONVIF_PASSWORD`
- `MOQT_URL` when publishing to a MoQT relay

```shell
cp .env.example .env
```

## Run

Run the GUI controller:

```shell
make onvif-controller
```

Run the MoQ bridge:

```shell
make onvif
```

`make onvif` publishes RTSP video/audio as profile-specific MoQT tracks and subscribes to ONVIF command datagrams.

## Direct CLI Options

Use `cargo run` directly when you need options that are not exposed by the Makefile helpers.

GUI controller:

```shell
cargo run -p moqt-bridge-onvif --bin moqt-bridge-onvif -- \
  --ip 192.168.11.45 \
  --username admin \
  --password secret
```

MoQ bridge:

```shell
cargo run -p moqt-bridge-onvif --bin moqt-onvif-client -- \
  --ip 192.168.11.45 \
  --username admin \
  --password secret \
  --moqt-url https://127.0.0.1:4433 \
  --payload-format avcc \
  --dump-keyframe \
  --insecure-skip-tls-verify
```

Common options:

- `--ip`: camera IP address or hostname
- `--username`, `--password`: ONVIF credentials
- `--moqt-url`: MoQT relay URL
- `--payload-format`: `annexb` or `avcc`
- `--insecure-skip-tls-verify`: skip TLS certificate verification for local development
  only
- `--dump-keyframe[=PATH]`: dump the first keyframe payload for ffprobe

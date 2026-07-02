# moqt-bridge-live-ingest

Live ingest bridge for publishing RTMP or SRT media into MoQT.

## Run

Run the bridge:

```shell
make live-ingest
```

`make live-ingest` listens for RTMP on `0.0.0.0:1935`, listens for SRT on
`0.0.0.0:9000`, and publishes to the local MoQT relay. On macOS with the
Docker Compose relay running, the relay URL is resolved to the Docker Desktop
bridge host automatically.

Override the relay URL when needed:

```shell
LIVE_INGEST_MOQT_URL=https://relay.example.com:443 make live-ingest
```

## Publish Test RTMP

Publish a generated test video and sine audio stream:

```shell
make ffmpeg-rtmp
```

## Direct CLI Options

Use `cargo run` directly when you need options that are not exposed by the Makefile helpers.

```shell
cargo run -p moqt-bridge-live-ingest -- \
  --rtmp-addr 0.0.0.0:1935 \
  --srt-addr 0.0.0.0:9000 \
  --moqt-url https://127.0.0.1:4433
```

Options:

- `--rtmp-addr`: RTMP listen address
- `--srt-addr`: SRT listen address
- `--moqt-url`: MoQT relay URL

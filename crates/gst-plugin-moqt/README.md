# gst-plugin-moqt

GStreamer sink plugin (`moqtsink`) that publishes H.264 video and AAC audio from a GStreamer pipeline to a MoQT relay as an MSF broadcast (LOC + CMAF tracks via [`crates/broadcast`](../broadcast/README.md)).

## Requirements & build

Requires GStreamer 1.20+ with `gst-plugins-base`, `gst-plugins-good`, and `gst-plugins-bad` (`brew install gstreamer` on macOS; `pkg-config`, `libgstreamer1.0-dev`, `gstreamer1.0-tools`, `gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`, `gstreamer1.0-plugins-bad` on Debian/Ubuntu).

Build and verify the plugin:

```shell
make gst-plugin
gst-inspect-1.0 --gst-plugin-load=target/debug/libgstmoqt.dylib moqtsink  # macOS
gst-inspect-1.0 --gst-plugin-load=target/debug/libgstmoqt.so moqtsink     # Linux
```

## Quickstart: Publish an SRT stream to a local relay

1. Run each command in its own terminal from the repository root (`ffmpeg` provides the SRT test source):

```shell
make relay                    # local MoQT relay on https://127.0.0.1:4433
make gst-srt-publish          # SRT listener on 0.0.0.0:9000 -> anon/live/test
make ffmpeg-srt               # push test pattern over SRT (or make ffmpeg-srt-bbb-local)
```

2. Open **Live Viewer** in MoQT Playground and watch `anon/live/test`.

`make gst-srt-publish` runs (configurable via `GST_MOQT_URL`, `GST_SRT_ADDR`, `GST_NAMESPACE`):

```shell
gst-launch-1.0 --gst-plugin-load=target/debug/libgstmoqt.dylib -e \
  srtsrc uri="srt://0.0.0.0:9000?mode=listener" ! tsdemux name=demux \
  demux. ! queue ! h264parse config-interval=-1 ! moqt. \
  demux. ! queue ! aacparse ! moqt. \
  moqtsink name=moqt relay-url=https://127.0.0.1:4433 insecure-skip-tls-verify=true namespace=anon/live/test
```

## Request sink pads

Each pad may be requested at most once:

| Pad | Required caps | Recommended upstream element |
| --- | --- | --- |
| `video` | `video/x-h264, stream-format=byte-stream, alignment=au` | `h264parse config-interval=-1` (in-band SPS/PPS on keyframes) |
| `audio` | `audio/mpeg, mpegversion=4, stream-format=raw` (with `codec_data`) | `aacparse` (attaches `AudioSpecificConfig` in caps) |

## Properties

Configure all properties before transitioning out of `READY`:

| Property | Required | Default | Description |
| --- | --- | --- | --- |
| `relay-url` | Yes | — | `moqt://host:port` (QUIC) or `https://host:port` (WebTransport) |
| `namespace` | Yes | — | `/`-separated MoQT namespace (e.g. `anon/live/test`) |
| `auth-token` | No | — | JWT sent in `CLIENT_SETUP` (required outside `anon/**`) |
| `insecure-skip-tls-verify` | No | `false` | Skip TLS certificate verification (local self-signed relays only) |

## Behavior & caveats

- **Connection lifecycle**: Connects and announces on `READY -> PAUSED` (no preroll). Missing required properties or an unreachable relay fail pipeline startup immediately. Closes the relay session on `PAUSED -> READY`.
- **Timestamps**: Converts buffer PTS/DTS via pad segment running time so video and audio share one clock; buffers without a running time are dropped with `WARN`.
- **Queue & backpressure**: Buffers up to 2,310 slots (~30 s at 30 fps video + 48 kHz AAC; 8 slots reserved for configs). Logs a `WARN` once the backlog exceeds ~3 s; when full, drops frames until the next keyframe fits.
- **EOS & logging**: Posts pipeline EOS only after every requested pad reaches EOS. Logs go to stderr via `tracing` at `info` by default (override with `RUST_LOG`).

## E2E test & further reading

```shell
./tests/moqtsink-e2e/run.sh
```

- GStreamer usage note: [docs/architecture/gst-plugin-moqt/gstreamer.md](../../docs/architecture/gst-plugin-moqt/gstreamer.md)
- Underlying broadcast publisher: [`crates/broadcast`](../broadcast/README.md)

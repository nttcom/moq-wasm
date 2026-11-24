# gst-plugin-moqt

GStreamer sink plugin (`moqtsink`) that publishes H.264 video and AAC audio as an MSF broadcast ([`crates/broadcast`](../broadcast/README.md)) to a MoQT relay.

## Build & verify

Requires GStreamer 1.20+ with `gst-plugins-base`, `-good`, and `-bad` (`brew install gstreamer` on macOS; `pkg-config`, `libgstreamer1.0-dev`, `gstreamer1.0-tools`, `gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`, `gstreamer1.0-plugins-bad` on Debian/Ubuntu). The SRT sender step below also uses `ffmpeg`:

```shell
make gst-plugin
gst-inspect-1.0 --gst-plugin-load=target/debug/libgstmoqt.dylib moqtsink  # Linux: libgstmoqt.so
```

## Quickstart: Publish an SRT stream

Run each command in its own terminal from the repository root, then watch `anon/live/test` in Live Viewer:

```shell
make relay
make gst-srt-publish          # SRT listener on 0.0.0.0:9000 -> anon/live/test
make ffmpeg-srt-bbb-local     # or make ffmpeg-srt
```

`make gst-srt-publish` runs (override with `GST_MOQT_URL`, `GST_SRT_ADDR`, and `GST_NAMESPACE`):

```shell
gst-launch-1.0 --gst-plugin-load=target/debug/libgstmoqt.dylib -e \
  srtsrc uri="srt://0.0.0.0:9000?mode=listener" ! tsdemux name=demux \
  demux. ! queue ! h264parse config-interval=-1 ! moqt. \
  demux. ! queue ! aacparse ! moqt. \
  moqtsink name=moqt relay-url=https://127.0.0.1:4433 insecure-skip-tls-verify=true namespace=anon/live/test
```

## Pads, properties & behaviour

| Request sink pad | Caps | Required upstream element |
| --- | --- | --- |
| `video` | `video/x-h264, stream-format=byte-stream, alignment=au` | `h264parse config-interval=-1` (in-band SPS/PPS on keyframes) |
| `audio` | `audio/mpeg, mpegversion=4, stream-format=raw` | `aacparse` (provides `codec_data`) |

| Property | Required | Meaning |
| --- | --- | --- |
| `relay-url` | yes | `moqt://host:port` (QUIC) or `https://host:port` (WebTransport) |
| `namespace` | yes | `/`-separated namespace, e.g. `anon/live/test` |
| `auth-token` | no | JWT presented in `CLIENT_SETUP` (needed outside `anon/**`) |
| `insecure-skip-tls-verify` | no | Skip TLS verification (local self-signed relays only) |

- Connects on `PAUSED`, failing pipeline startup immediately if the relay is unreachable.
- Buffers ~3 s of media; under backpressure, drops frames until the next keyframe fits and logs drop counts at `WARN`.
- Logs via `tracing` at `info` (override with `RUST_LOG`).

## E2E

```shell
./tests/moqtsink-e2e/run.sh
```

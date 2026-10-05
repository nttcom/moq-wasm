# gst-plugin-moqt

GStreamer plugin with a `moqtsink` element that publishes H.264 video and AAC
audio into a MoQT relay. It produces the tracks described in
[`crates/media-publisher`](../media-publisher/README.md), so the
[Live Viewer](../../examples/browser/examples/live-viewer/README.md) plays and
rewinds them like a `live-ingest` stream.

## Prerequisites

GStreamer 1.20+ with `gst-plugins-base`, `gst-plugins-good` and
`gst-plugins-bad` (`srtsrc`, `tsdemux`, `h264parse`, `aacparse`). On macOS
`brew install gstreamer` installs all of them.

## Build

```shell
make gst-plugin
GST_PLUGIN_PATH=target/debug gst-inspect-1.0 moqtsink
```

## Element

Request sink pads:

| Pad | Caps |
| --- | --- |
| `video` | `video/x-h264, stream-format=byte-stream, alignment=au` |
| `audio` | `audio/mpeg, mpegversion=4, stream-format=raw` |

Put `h264parse config-interval=-1` in front of the video pad so every keyframe
carries its SPS/PPS, and `aacparse` in front of the audio pad so the caps carry
`codec_data`.

Properties:

| Property | Meaning |
| --- | --- |
| `relay-url` | `moqt://host:port` for QUIC or `https://host:port` for WebTransport (required) |
| `namespace` | Slash-separated track namespace, e.g. `anon/live/test` (required) |
| `auth-token` | JWT presented to the relay in CLIENT_SETUP |

Behaviour:

- Connects and publishes when the pipeline goes to PAUSED, so a wrong relay URL fails the pipeline start.
- Buffers about three seconds of media in a queue drained by a background task; the streaming thread never waits for the relay.
- When the relay does not keep up, drops media until the next keyframe fits and logs the dropped counts at WARN. The pipeline fails only when the relay connection is lost.
- Logs through `tracing` at `info`; set `RUST_LOG` to change it (e.g. `RUST_LOG=media_publisher=debug`).

## SRT to MoQT

```shell
make relay
make gst-srt-publish          # SRT listener on 0.0.0.0:9000 -> anon/live/test
make ffmpeg-srt-bbb-local     # or make ffmpeg-srt
```

`make gst-srt-publish` runs:

```shell
GST_PLUGIN_PATH=target/debug gst-launch-1.0 -e \
  srtsrc uri="srt://0.0.0.0:9000?mode=listener" ! tsdemux name=demux \
  demux. ! queue ! h264parse config-interval=-1 ! moqt. \
  demux. ! queue ! aacparse ! moqt. \
  moqtsink name=moqt relay-url=https://127.0.0.1:4433 namespace=anon/live/test
```

`GST_MOQT_URL`, `GST_SRT_ADDR` and `GST_NAMESPACE` override the relay, the SRT
listen address and the namespace.

## E2E

```shell
./scripts/moqtsink-e2e.sh
```

Publishes a test pattern and tone through `moqtsink` into a Docker Compose
relay and checks the tracks from a subscriber.

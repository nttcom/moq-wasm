# gst-plugin-moqt

GStreamer plugin with a `moqtsink` element that publishes H.264 video and AAC
audio into a MoQT relay. The tracks it publishes are the same as
`bridges/live-ingest` produces (`shared/media-publisher`): LOC `video` /
`audio` tracks with their `_cmaf` siblings, an MSF `catalog`, a media
`timeline` and a 30-second publisher-side FETCH cache, so
`examples/browser/examples/live-viewer` plays and rewinds them unchanged.

## Prerequisites

GStreamer 1.20+ with `gst-plugins-base`, `gst-plugins-good` and
`gst-plugins-bad` (`srtsrc`, `tsdemux`, `h264parse`, `aacparse`). On macOS
`brew install gstreamer` installs all of them.

## Build

```shell
make gst-plugin
```

The plugin is written to `target/debug/libgstmoqt.{dylib,so}`; point
`GST_PLUGIN_PATH` at that directory:

```shell
GST_PLUGIN_PATH=target/debug gst-inspect-1.0 moqtsink
```

## Element

`moqtsink` has two request sink pads:

| Pad | Caps |
| --- | --- |
| `video` | `video/x-h264, stream-format=byte-stream, alignment=au` |
| `audio` | `audio/mpeg, mpegversion=4, stream-format=raw` |

Parameter sets must travel in band: put `h264parse config-interval=-1` in
front of the video pad so every keyframe carries its SPS/PPS. The audio caps
must carry `codec_data` (the AudioSpecificConfig); `aacparse` provides it when
converting ADTS to raw.

Properties:

- `relay-url`: `moqt://host:port` for QUIC or `https://host:port` for
  WebTransport (required)
- `namespace`: slash-separated track namespace, e.g. `anon/live/test`
  (required)
- `auth-token`: JWT presented to the relay in CLIENT_SETUP

The sink connects and publishes the namespace when the pipeline goes to
PAUSED, so a wrong relay URL fails the pipeline start instead of the first
buffer. Objects are numbered and cached from the first buffer on; a subscriber
that arrives while a group is open starts receiving at the next group.

## SRT to MoQT

```shell
make relay
make gst-srt-publish     # listens on 0.0.0.0:9000 and publishes anon/live/test
make ffmpeg-srt-bbb-local      # or make ffmpeg-srt
```

`make gst-srt-publish` runs:

```shell
GST_PLUGIN_PATH=target/debug gst-launch-1.0 -e \
  srtsrc uri="srt://0.0.0.0:9000?mode=listener" ! tsdemux name=demux \
  demux. ! queue ! h264parse config-interval=-1 ! moqt. \
  demux. ! queue ! aacparse ! moqt. \
  moqtsink name=moqt relay-url=https://127.0.0.1:4433 namespace=anon/live/test
```

Override `GST_MOQT_URL`, `GST_SRT_ADDR` and `GST_NAMESPACE` to change the
relay, the SRT listen address and the namespace. Watch the stream in the
browser with `make browser` and `make chrome`, then open
`examples/live-viewer/index.html` with the namespace `anon/live/test`; the
rewind controls FETCH the last 30 seconds from the relay or, for groups the
relay never cached, from the sink itself.

Logs go through `tracing` at `info` by default; set `RUST_LOG` to change the
filter (for example `RUST_LOG=media_publisher=debug`).

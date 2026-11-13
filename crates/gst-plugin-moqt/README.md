# gst-plugin-moqt

GStreamer plugin with a `moqtsink` element that publishes H.264 video and AAC
audio into a MoQT relay, as the tracks described in
[`crates/broadcast`](../broadcast/README.md).

Needs GStreamer 1.20+ with `gst-plugins-base`, `-good` and `-bad`
(`brew install gstreamer` on macOS).

```shell
make gst-plugin
gst-inspect-1.0 --gst-plugin-load=target/debug/libgstmoqt.dylib moqtsink
```

## Element

| Request sink pad | Caps | Put in front |
| --- | --- | --- |
| `video` | `video/x-h264, stream-format=byte-stream, alignment=au` | `h264parse config-interval=-1`, so every keyframe carries its SPS/PPS |
| `audio` | `audio/mpeg, mpegversion=4, stream-format=raw` | `aacparse`, so the caps carry `codec_data` |

| Property | Meaning |
| --- | --- |
| `relay-url` | `moqt://host:port` for QUIC or `https://host:port` for WebTransport (required) |
| `namespace` | Slash-separated track namespace, e.g. `anon/live/test` (required) |
| `auth-token` | JWT presented to the relay in CLIENT_SETUP |
| `insecure-skip-tls-verify` | Skip TLS certificate verification, for a local relay with a self-signed certificate only |

- Connects when the pipeline goes to PAUSED, so a wrong relay URL fails the pipeline start.
- Buffers about three seconds of media; when the relay does not keep up, it drops media until the next keyframe fits and logs the counts at WARN.
- Logs through `tracing` at `info`; `RUST_LOG` changes it.

## SRT to MoQT

```shell
make relay
make gst-srt-publish          # SRT listener on 0.0.0.0:9000 -> anon/live/test
make ffmpeg-srt-bbb-local     # or make ffmpeg-srt
```

`make gst-srt-publish` runs:

```shell
gst-launch-1.0 --gst-plugin-load=target/debug/libgstmoqt.dylib -e \
  srtsrc uri="srt://0.0.0.0:9000?mode=listener" ! tsdemux name=demux \
  demux. ! queue ! h264parse config-interval=-1 ! moqt. \
  demux. ! queue ! aacparse ! moqt. \
  moqtsink name=moqt relay-url=https://127.0.0.1:4433 insecure-skip-tls-verify=true namespace=anon/live/test
```

`GST_MOQT_URL`, `GST_SRT_ADDR` and `GST_NAMESPACE` override the relay, the SRT
listen address and the namespace.

## E2E

```shell
./tests/moqtsink-e2e/run.sh
```

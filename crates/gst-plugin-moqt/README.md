# gst-plugin-moqt

GStreamer sink plugin (`moqtsink`) that publishes H.264 video and AAC audio from a GStreamer pipeline to a MoQT relay as an MSF broadcast (LOC + CMAF tracks via [`crates/broadcast`](../broadcast/README.md)).

## Requirements

Requires GStreamer 1.20+ with `gst-plugins-base`, `gst-plugins-good`, and `gst-plugins-bad` for the elements upstream of `moqtsink` (`h264parse`, `aacparse`, …): `brew install gstreamer` on macOS; `gstreamer1.0-tools`, `gstreamer1.0-plugins-base`, `gstreamer1.0-plugins-good`, `gstreamer1.0-plugins-bad` on Debian/Ubuntu; the [official MSVC installer](https://gstreamer.freedesktop.org/download/) on Windows.

## Install

Each `gst-plugin-moqt` release on the [releases page](https://github.com/nttcom/moq-wasm/releases) has one archive per platform holding the plugin and `LICENSE`. Put the plugin in GStreamer's user plugin directory:

| Platform | Archive | Built against | Plugin directory |
| --- | --- | --- | --- |
| macOS (Apple silicon) | `gst-plugin-moqt-<version>-macos-arm64.tar.gz` | Homebrew's `gstreamer` | `~/.local/share/gstreamer-1.0/plugins` |
| Linux (x86_64 / aarch64) | `gst-plugin-moqt-<version>-linux-<arch>.tar.gz` | Ubuntu 22.04 (glibc 2.35+, GStreamer 1.20+) | `~/.local/share/gstreamer-1.0/plugins` |
| Windows (x64) | `gst-plugin-moqt-<version>-windows-x64.zip` | Official MSVC GStreamer | `%LOCALAPPDATA%\gstreamer-1.0\plugins` |

On macOS and Linux (set `VERSION` and the archive's platform):

```shell
VERSION=0.1.0 PLATFORM=macos-arm64   # or linux-x86_64, linux-aarch64
mkdir -p ~/.local/share/gstreamer-1.0/plugins
curl -fL "https://github.com/nttcom/moq-wasm/releases/download/gst-plugin-moqt-v${VERSION}/gst-plugin-moqt-${VERSION}-${PLATFORM}.tar.gz" \
  | tar -xz -C ~/.local/share/gstreamer-1.0/plugins --exclude LICENSE
gst-inspect-1.0 moqtsink
```

On Windows, extract `gstmoqt.dll` from the zip into `%LOCALAPPDATA%\gstreamer-1.0\plugins` (create the folder if it is missing) and run `gst-inspect-1.0 moqtsink`.

- **macOS**: The plugin is not signed. `curl` leaves no quarantine on it; if a browser downloaded the archive, run `xattr -d com.apple.quarantine ~/.local/share/gstreamer-1.0/plugins/libgstmoqt.dylib`, or GStreamer fails to load it. The official `GStreamer.framework` is not supported.
- **Other directories**: `GST_PLUGIN_PATH` adds any directory holding the plugin.
- **Uninstall**: Delete the plugin file from the plugin directory.

## Build from source

Additionally requires `pkg-config` and the GStreamer development files (`libgstreamer1.0-dev` on Debian/Ubuntu; included in Homebrew's `gstreamer` and the official Windows installer's development setup). Build and verify the plugin:

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

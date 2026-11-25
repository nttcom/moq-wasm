# obs-moqt

OBS Studio plugin that publishes the stream OBS encodes (H.264 video, AAC audio) straight to a MoQT relay as an MSF broadcast (LOC + CMAF tracks via [`crates/broadcast`](../broadcast/README.md)). It registers the `moqt_output` output and the `moqt_service` streaming service, so **Start Streaming**, the encoder settings, the status bar, and automatic reconnect all work as they do for RTMP or SRT.

## Requirements & build

Requires OBS Studio 32.0 or newer (tested with 32.0.2 on macOS). Nothing from OBS is needed at build time: libobs symbols are resolved when OBS loads the plugin.

```shell
make obs-plugin            # cargo build -p obs-moqt --release
make obs-plugin-install    # macOS: copies it into OBS's user plugin directory
```

`make obs-plugin-install` creates `~/Library/Application Support/obs-studio/plugins/obs-moqt.plugin/Contents/MacOS/obs-moqt`. On Linux (not tested yet), copy `target/release/libobs_moqt.so` to `~/.config/obs-studio/plugins/obs-moqt/bin/64bit/obs-moqt.so`. Restart OBS afterwards; **Help → Log Files → View Current Log** lists `obs-moqt` among the loaded modules.

## Select the MoQT service

OBS does not yet list services that plugins register under **Settings → Stream**, so the service is selected in one of two other ways. Both keep the choice in the profile.

- **Profile file**: quit OBS and write the profile's `service.json` (`~/Library/Application Support/obs-studio/basic/profiles/<profile>/service.json` on macOS, `~/.config/obs-studio/basic/profiles/<profile>/service.json` on Linux):

  ```json
  {
    "type": "moqt_service",
    "settings": {
      "server": "moqt://127.0.0.1:4433",
      "namespace": "anon/obs/live",
      "insecure_skip_tls_verify": true
    }
  }
  ```

- **obs-websocket** (**Tools → WebSocket Server Settings**), while OBS runs: send `SetStreamServiceSettings` with `streamServiceType` `moqt_service` and the same settings as `streamServiceSettings`.

Then press **Start Streaming** and watch the namespace in **Live Viewer**. The **Settings → Stream** page does not show the service, and saving a change there replaces it with the one chosen on the page.

## Service settings

| Setting | Required | Default | Description |
| --- | --- | --- | --- |
| `server` | Yes | — | `moqt://host:port` (QUIC) or `https://host:port` (WebTransport) |
| `namespace` | Yes | — | `/`-separated MoQT namespace (e.g. `anon/obs/live`) |
| `bearer_token` | No | — | JWT sent in `CLIENT_SETUP` (required outside `anon/**`) |
| `insecure_skip_tls_verify` | No | `false` | Skip TLS certificate verification (local self-signed relays only) |

## Behavior & caveats

- **Codecs**: H.264 and AAC only; choose an H.264 video encoder and AAC audio under **Settings → Output**. When the encoder settings leave the keyframe interval at 0 (always the case in simple output mode), it is set to 2 s so that viewers join quickly.
- **Connection lifecycle**: **Start Streaming** connects and announces the namespace; a missing setting or an unreachable relay ends the attempt, and OBS reports the reason (also in its log). A relay lost mid-stream is reported as a disconnect once the transport times out (about 30 s on QUIC), which OBS answers with its reconnect settings (**Settings → Advanced → Automatically Reconnect**).
- **Timestamps**: Video and audio share one clock that starts at the first video packet's decode time, so B-frames keep their presentation order. Audio that precedes the first video packet is left out.
- **Queue & backpressure**: Same publish queue as the other `broadcast` publishers (~30 s; when full, frames are dropped until the next keyframe fits). Dropped video frames show up as OBS's dropped frames.
- **Logging**: `tracing` logs go to stderr at `info` by default (override with `RUST_LOG`); start OBS from a terminal to see them.

## License

GPL-2.0-or-later, because the plugin links libobs. The rest of this repository stays under the MIT license.

## Further reading

- Design notes: [docs/architecture/obs-moqt/architecture.md](../../docs/architecture/obs-moqt/architecture.md)
- Underlying broadcast publisher: [`crates/broadcast`](../broadcast/README.md)

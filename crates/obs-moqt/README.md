# obs-moqt

OBS Studio plugin that publishes the stream OBS encodes (H.264 video, AAC audio) straight to a MoQT relay as an MSF broadcast (LOC + CMAF tracks via [`crates/broadcast`](../broadcast/README.md)). It registers the `moqt_output` output and the `moqt_service` streaming service, so **Start Streaming**, the encoder settings, the status bar, and automatic reconnect all work as they do for RTMP or SRT. A **MoQT** dock holds the settings.

## Install

Requires OBS Studio 32.0 or newer (tested with 32.0.2 on macOS). An installer exists for macOS on Apple silicon; on other platforms, [build from source](#build-from-source).

1. Download `obs-moqt-<version>-macos-arm64.pkg` from the newest `obs-moqt` release on the [releases page](https://github.com/nttcom/moq-wasm/releases).
2. Open it. The installer is not signed, so macOS refuses the first time: open **System Settings → Privacy & Security**, press **Open Anyway** next to the installer's name, and open it again.
3. Follow the installer. It puts `obs-moqt.plugin` into `~/Library/Application Support/obs-studio/plugins` and asks for no administrator password.
4. Restart OBS and tick **Docks → MoQT**.

To uninstall, delete `~/Library/Application Support/obs-studio/plugins/obs-moqt.plugin`.

## Build from source

Nothing from OBS is needed at build time: libobs and Qt symbols are resolved when OBS loads the plugin.

```shell
make obs-plugin            # cargo build -p obs-moqt --release
make obs-plugin-install    # macOS: copies it into OBS's user plugin directory
make obs-plugin-pkg        # macOS: builds the installer under target/obs-plugin-pkg
```

The dock is C++ and needs a C++17 compiler and Qt 6 headers no newer than the Qt in OBS (6.8 in OBS 32.0). `make obs-plugin` takes the compiler flags that find them from `pkg-config --cflags Qt6Widgets`, or from `OBS_MOQT_QT_CFLAGS`:

```shell
# macOS, with the Qt package OBS itself is built with
# (obs-deps release named in OBS's buildspec.json, unpacked to $QT):
make obs-plugin-install OBS_MOQT_QT_CFLAGS="-F$QT/lib"
```

Without those flags, and in a plain `cargo build`, the plugin builds without the dock and the service is selected as in [Without the dock](#without-the-dock).

`make obs-plugin-install` creates `~/Library/Application Support/obs-studio/plugins/obs-moqt.plugin/Contents/MacOS/obs-moqt`. On Linux (not tested yet), copy `target/release/libobs_moqt.so` to `~/.config/obs-studio/plugins/obs-moqt/bin/64bit/obs-moqt.so`. Restart OBS afterwards; **Help → Log Files → View Current Log** lists `obs-moqt` among the loaded modules.

## Select the MoQT service

OBS does not yet list services that plugins register under **Settings → Stream**, so the plugin adds its own dock:

1. Open **Docks → MoQT**.
2. Fill in the [service settings](#service-settings) and press **Use for streaming**. The dock saves them in the profile as its stream destination, or says which field is missing.
3. Press **Start Streaming** and watch the namespace in **Live Viewer**.

The destination cannot be changed while a stream runs. The **Settings → Stream** page does not show the service, and saving a change there replaces it with the one chosen on the page.

### Without the dock

The same settings can be written in one of two other ways. Both keep the choice in the profile.

- **Profile file**: quit OBS and write the profile's `service.json` (`~/Library/Application Support/obs-studio/basic/profiles/<profile>/service.json` on macOS, `~/.config/obs-studio/basic/profiles/<profile>/service.json` on Linux):

  ```json
  {
    "type": "moqt_service",
    "settings": {
      "server": "moqt://127.0.0.1:4433",
      "namespace": "obs/live",
      "bearer_token": "<JWT>",
      "insecure_skip_tls_verify": true
    }
  }
  ```

- **obs-websocket** (**Tools → WebSocket Server Settings**), while OBS runs: send `SetStreamServiceSettings` with `streamServiceType` `moqt_service` and the same settings as `streamServiceSettings`.

## Service settings

The dock's fields, in order, are `server`, `namespace`, `bearer_token`, `bearer_token_file`, and `insecure_skip_tls_verify`.

| Setting | Required | Default | Description |
| --- | --- | --- | --- |
| `server` | Yes | — | `moqt://host:port` (QUIC) or `https://host:port` (WebTransport). Without a port, `moqt://` goes to 4433 and `https://` to 443; the hosted evaluation relay listens on 443 (`moqt://relay.moqt.research.skyway.io:443`) |
| `namespace` | Yes | — | `/`-separated MoQT namespace under the token's app (e.g. `obs/live`). The plugin puts the token's `appId` in front, so viewers find the stream at `<appId>/obs/live`; a namespace that already starts with the `appId` is used as written |
| `bearer_token` | One of the two | — | JWT sent in `CLIENT_SETUP`, with a `publish` claim that covers the namespace (mint one with [`auth-token`](../auth-token/README.md)) |
| `bearer_token_file` | One of the two | — | Path of a file that holds the JWT. It is read at each start and every 10 s while streaming, so a stream can outlast its first token |
| `insecure_skip_tls_verify` | No | `false` | Skip TLS certificate verification (local self-signed relays only) |

## Behavior & caveats

- **Codecs**: H.264 and AAC only; choose an H.264 video encoder and AAC audio under **Settings → Output**. When the encoder settings leave the keyframe interval at 0 (always the case in simple output mode), it is set to 2 s so that viewers join quickly.
- **Connection lifecycle**: **Start Streaming** connects and announces the namespace; a missing setting, an unreachable relay, or a token that does not cover the namespace ends the attempt, and OBS reports the reason (also in its log). A relay lost mid-stream is reported as a disconnect once the transport times out (about 30 s on QUIC), which OBS answers with its reconnect settings (**Settings → Advanced → Automatically Reconnect**).
- **Token expiry**: The relay closes the session when the token expires (relays accept at most 24 h by default), and OBS's reconnects fail until the settings give a new token. With `bearer_token`, mint a token that outlasts the stream; OBS keeps it in the profile's `service.json` as plain text, like a stream key. With `bearer_token_file`, have a job replace the file (write a new file, then rename it over the old one) before the token expires: the plugin sends the relay the new token within 10 s. A file that is unreadable or holds a token the relay refuses leaves the current token in force and is read again 10 s later.
- **Timestamps**: Video and audio share one clock that starts at the first video packet's decode time, so B-frames keep their presentation order. Audio that precedes the first video packet is left out.
- **Queue & backpressure**: Same publish queue as the other `broadcast` publishers (~30 s; when full, frames are dropped until the next keyframe fits). Dropped video frames show up as OBS's dropped frames.
- **Logging**: `tracing` logs go to stderr at `info` by default (override with `RUST_LOG`); start OBS from a terminal to see them.

## License

GPL-3.0-or-later ([LICENSE](LICENSE)). The rest of this repository is MIT.

- **Why GPL**: OBS Studio is GPL (version 2 or later), and a plugin that OBS loads and that calls libobs is distributed under the GPL too.
- **Why version 3**: The TLS library built into the plugin (`aws-lc-sys`) contains Apache-2.0 code, which the GPL accepts from version 3 on.
- **What it covers**: The plugin binary as a whole, including the workspace crates built into it (`broadcast`, `moqt`, `mediapack`, `msf`). Those crates stay MIT for every other use, and the relay and the other programs the plugin reaches over the network are not covered.
- **Distributing a build**: Ship [LICENSE](LICENSE) with the binary and offer the source of everything built into it, local changes to the workspace crates included.

## Further reading

- Design notes: [docs/architecture/obs-moqt/architecture.md](../../docs/architecture/obs-moqt/architecture.md)
- Underlying broadcast publisher: [`crates/broadcast`](../broadcast/README.md)

# obs-moqt

OBS Studio plugin that publishes the stream OBS encodes (H.264 video, AAC audio) straight to a MoQT relay as an MSF broadcast (LOC + CMAF tracks via [`crates/broadcast`](../broadcast/README.md)). It registers the `moqt_output` output and the `moqt_service` streaming service, so **Start Streaming**, the encoder settings, the status bar, and automatic reconnect all work as they do for RTMP or SRT. A **MoQT** dock holds the settings.

## Install

Requires OBS Studio 32.0 or newer. Download the package for your platform from the newest `obs-moqt` release on the [releases page](https://github.com/nttcom/moq-wasm/releases), install it as below, then restart OBS and tick **Docks → MoQT**. **Help → Log Files → View Current Log** lists `obs-moqt` among the loaded modules.

| Platform | Package | Install | Uninstall |
| --- | --- | --- | --- |
| macOS (Apple silicon) | `obs-moqt-<version>-macos-arm64.pkg` | Open the installer. It asks for no administrator password | Delete `~/Library/Application Support/obs-studio/plugins/obs-moqt.plugin` |
| Windows (x64) | `obs-moqt-<version>-windows-x64.zip` | Extract its `obs-moqt` folder into `C:\ProgramData\obs-studio\plugins` (create the folder if it is missing) | Delete `C:\ProgramData\obs-studio\plugins\obs-moqt` |
| Linux (x86_64) | `obs-moqt-<version>-linux-x86_64.tar.gz` | `mkdir -p ~/.config/obs-studio/plugins && tar -xzf obs-moqt-<version>-linux-x86_64.tar.gz -C ~/.config/obs-studio/plugins` | Delete `~/.config/obs-studio/plugins/obs-moqt` |

- **macOS**: The installer is not signed, so macOS refuses it the first time: open **System Settings → Privacy & Security**, press **Open Anyway** next to the installer's name, and open it again.
- **Linux**: Built on Ubuntu 24.04 for the OBS of the [`obsproject` PPA](https://launchpad.net/~obsproject/+archive/ubuntu/obs-studio). Other distributions and the Flatpak are not tested.
- **Tested**: Streaming is tested on macOS (OBS 32.0.2). On Windows (OBS 32.0.2) and Linux (the PPA's OBS), each build is only checked to load in OBS.

## Build from source

```shell
make obs-plugin            # cargo build -p obs-moqt --release
make obs-plugin-install    # macOS: copies it into OBS's user plugin directory
make obs-plugin-pkg        # macOS: builds the installer under target/obs-plugin-pkg
```

On Linux, copy `target/release/libobs_moqt.so` to `~/.config/obs-studio/plugins/obs-moqt/bin/64bit/obs-moqt.so`; on Windows, `target\release\obs_moqt.dll` to `C:\ProgramData\obs-studio\plugins\obs-moqt\bin\64bit\obs-moqt.dll`.

Nothing from OBS is needed at build time. The dock is C++ and needs a C++17 compiler and Qt 6 headers no newer than the Qt in OBS; `OBS_MOQT_QT_CFLAGS` holds the compiler flags that find them:

| Platform | Qt | Flags |
| --- | --- | --- |
| macOS | The Qt package OBS is built with (the obs-deps release named in OBS's `buildspec.json`), unpacked to `$QT` | `make obs-plugin OBS_MOQT_QT_CFLAGS="-F$QT/lib"` |
| Linux | The distribution's `qt6-base-dev` | None: `make obs-plugin` asks `pkg-config --cflags Qt6Widgets` |
| Windows (MSVC) | The same obs-deps Qt package for Windows, unpacked to `%QT%` | `OBS_MOQT_QT_CFLAGS=-I%QT%/include` and `OBS_MOQT_QT_LIB_DIR=%QT%/lib`, then `cargo build -p obs-moqt --release` |

Without `OBS_MOQT_QT_CFLAGS`, as in a plain `cargo build`, the plugin builds without the dock and the service is selected as in [Without the dock](#without-the-dock).

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
- **What a package carries**: [LICENSE](LICENSE), [COPYRIGHT](COPYRIGHT) (the plugin's copyright notice and where its source is), and `THIRD-PARTY-LICENSES.txt`, the licenses of the libraries built into the plugin. `make target/obs-plugin-notices/THIRD-PARTY-LICENSES.txt` generates that list with [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) and fails on a library whose license [`about.toml`](about.toml) does not allow.
- **Distributing a build**: Ship those three files with the binary and offer the source of everything built into it, local changes to the workspace crates included.

## Further reading

- Design notes: [docs/architecture/obs-moqt/architecture.md](../../docs/architecture/obs-moqt/architecture.md)
- Underlying broadcast publisher: [`crates/broadcast`](../broadcast/README.md)

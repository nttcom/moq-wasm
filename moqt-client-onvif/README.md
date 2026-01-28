# moqt-client-onvif

An ONVIF PTZ client with a minimal GUI and RTSP preview.

## What it does now

- PTZ: Absolute/Relative/Continuous/Stop + Center with per-command pan/tilt/zoom/speed inputs under the command row.
- RTSP: display the live preview in the GUI.
- Startup: fetch PTZ configuration/option info and print a short summary (token/spaces/limits) grouped by `[GetToken]`, etc.

MoQ publishing is not implemented yet.

## Usage

```shell
cargo run -p moqt-client-onvif -- \
  --ip 192.168.11.45 \
  --username admin \
  --password secret
```

Use the command buttons in the top row, then edit the pan/tilt/zoom/speed fields for each command below.

## Options

- `--ip`: camera IP address or hostname (required)
- `--username`, `--password`: credentials for ONVIF WS-Security (required)
- `--timeout-ms`: timeout per operation

## Notes

- ONVIF device service endpoint is fixed to `http://{ip}:2020/onvif/device_service`.
- PTZ profile token is selected from the first `GetProfiles` entry.
- PTZ/Media endpoints are discovered via `GetServices`/`GetCapabilities` (fallback to device service).
- Endpoint discovery tolerates shared XAddr values between media and PTZ services.
- PTZ configuration token is taken from `GetConfigurations` (profile token is used only if it appears there).
- If `GetConfiguration` reports `NoEntity`, `GetNodes`/`GetNode` are queried instead and summarized (`PTZ nodes`, `PTZ node`, `PTZ spaces`).
- `GetConfigurations` is summarized (`PTZ config`, `PTZ config spaces`, `PTZ pan/tilt limits`, `PTZ default speed`, `PTZ timeout`).
- Startup flow: `GetServices`/`GetCapabilities` → `GetProfiles` → PTZ configuration queries.
- PTZ ranges/speed defaults are derived from `GetConfigurations` + `GetConfigurationOptions` when available (fallback to -1.0..1.0 and 0.0..1.0).
- Implementation modules: onvif_command (SOAP ops builder + constants), onvif_client (endpoint holder + sender + init flow), ptz_panel (GUI controls), viewer (GUI app).
- ptz_panel is split into inputs/layout submodules to separate parsing and UI layout.
- PTZ GUI commands are laid out in a row with per-command input columns and enumerated via `strum` EnumIter for consistency.
- PTZ GUI input values snap to 0.1 increments.
- RTSP endpoint is fixed to `rtsp://{username}:{password}@{ip}:554/stream1`.
- GUI button presses log the command; SOAP responses log status only (no body).
- PTZ configuration queries (`GetConfigurations`, `GetConfiguration`, `GetConfigurationOptions`) log status on startup.
- Logs are emitted via `log` + `env_logger` with timestamp + level (set `RUST_LOG=info|debug`).
- Building requires FFmpeg libraries available for `ffmpeg-next`.
- PTZ controls are pinned to the bottom so the video never covers them.
- PTZ ranges/options are parsed during initialization (`ptz_config::extract_range_from_config` + `ptz_config::update_range_from_options`) and shown in the GUI.
- PTZ node capabilities from `GetNodes`/`GetNode` are stored and shown in the GUI, and unsupported commands are disabled when spaces are known.
- PTZ range + node data are bundled as a single PTZ state update when passed to the GUI.
- PTZ/ONVIF constants are currently hardcoded in `moqt-client-onvif/src/onvif_command.rs` (later we can derive them from `GetConfigurations`).

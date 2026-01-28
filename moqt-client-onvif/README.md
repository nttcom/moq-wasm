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

# moqt-client-onvif

An ONVIF PTZ client with a minimal CLI.

## What it does now

- PTZ: interactive pan/tilt control with arrow keys (AbsoluteMove).

MoQ publishing is not implemented yet.

## Usage

```shell
cargo run -p moqt-client-onvif -- \
  --ip 192.168.11.10 \
  --username admin \
  --password secret
```

Arrow keys move, space centers, and `q`/`Esc`/`Ctrl+C` quits.

## Options

- `--ip`: camera IP address or hostname (required)
- `--username`, `--password`: credentials for ONVIF WS-Security (required)
- `--timeout-ms`: timeout per operation

## Notes

- ONVIF device service endpoint is fixed to `http://{ip}:2020/onvif/device_service`.
- PTZ mode prints the selected Media/PTZ service endpoints.
- PTZ absolute moves use fixed ranges and steps (-1.0..1.0 with step 0.1 for pan/tilt).

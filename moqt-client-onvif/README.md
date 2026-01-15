# moqt-client-onvif

A small client for connecting to IP cameras via RTSP and ONVIF.

## What it does now

- RTSP: sends an `OPTIONS` request to confirm the RTSP endpoint is reachable.
- ONVIF: sends `GetDeviceInformation` to the device service, reports HTTP status, and prints the response body.
- PTZ: interactive pan/tilt control with arrow keys (AbsoluteMove).

MoQ publishing is not implemented yet.

## Usage

```shell
cargo run -p moqt-client-onvif -- --ip 192.168.1.10
```

With authentication and custom paths:

```shell
cargo run -p moqt-client-onvif -- \
  --ip 192.168.1.10 \
  --username admin \
  --password secret \
  --rtsp-path /stream1 \
  --onvif-path /onvif/device_service
```

With WS-Security UsernameToken:

```shell
cargo run -p moqt-client-onvif -- \
  --ip 192.168.1.10 \
  --username admin \
  --password secret \
  --onvif-auth wsse
```

Interactive PTZ control:

```shell
cargo run -p moqt-client-onvif -- \
  --ip 192.168.1.10 \
  --username admin \
  --password secret \
  --onvif-auth wsse \
  --ptz
```
Arrow keys move, space centers, and `q`/`Esc`/`Ctrl+C` quits.

## Options

- `--ip`: camera IP address or hostname (required)
- `--username`, `--password`: credentials for RTSP/ONVIF (optional)
- `--rtsp-port`, `--rtsp-path`: RTSP endpoint overrides
- `--onvif-port`, `--onvif-path`: ONVIF device service overrides (default port: 2020)
- `--onvif-auth`: ONVIF auth mode (`basic` or `wsse`)
- `--onvif-insecure`: allow invalid TLS certificates for ONVIF HTTPS
- `--ptz`: enable interactive PTZ control (arrow keys)
- `--ptz-profile-token`: PTZ profile token override
- `--ptz-endpoint`: PTZ service endpoint override
- `--ptz-log-responses`: log PTZ SOAP responses
- `--timeout-ms`: timeout per operation

## Notes

- RTSP authentication uses the URL userinfo when both username and password are provided.
- ONVIF uses HTTP basic authentication when credentials are provided.
- Some devices require WS-Security UsernameToken instead of HTTP basic auth.
- PTZ mode prints the selected Media/PTZ service endpoints.
- When using HTTPS endpoints, `--onvif-insecure` allows self-signed certificates.
- PTZ requests retry via the device service endpoint on transport errors; you can also override with `--ptz-endpoint`.
- Use `--ptz-log-responses` to print PTZ SOAP response bodies for debugging.
- PTZ absolute moves use fixed ranges and steps (-1.0..1.0 with step 0.1 for pan/tilt).

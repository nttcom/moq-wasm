# moqt-client-onvif

An ONVIF PTZ client with a minimal GUI and RTSP preview.

## What it does now

- PTZ: Absolute/Relative/Continuous/Stop + Center with per-command pan/tilt/zoom/speed inputs under the command row.
- PTZ inputs snap to 0.1 and show out-of-range errors while editing.
- RTSP: display the live preview in the GUI.
- RTSP: log SDP details once at startup when available.
- Startup: fetch PTZ configuration/option info and print a short summary (token/spaces/limits) grouped by `[GetToken]`, etc.
- MoQ: publish RTSP video as subgroup streams and subscribe to ONVIF command datagrams over a single WebTransport connection (AnnexB conversion applied; keyframes include LoC videoConfig derived from SPS/PPS when available; codec string is derived from SPS with the CLI value used as a fallback; RTSP capture starts after MoQ setup/announce/subscribe; each group switch sends an `EndOfGroup` object and finishes the previous subgroup stream).

## Usage

```shell
cargo run -p moqt-client-onvif -- \
  --ip 192.168.11.45 \
  --username admin \
  --password secret
```

Use the command buttons in the top row, then edit the pan/tilt/zoom/speed fields for each command below.

### Makefile helper

Create `.env` from `.env.example` and set `ONVIF_IP` / `ONVIF_USERNAME` / `ONVIF_PASSWORD`.

```shell
cp .env.example .env
make onvif
```

`make onvif` runs the `moqt-client-onvif` GUI binary.

MoQ bridge runs with `MOQT_URL` from `.env`:

```shell
make onvif-moq
```

### MoQ bridge (moqt-onvif-client)

```shell
cargo run -p moqt-client-onvif --bin moqt-onvif-client -- \
  --ip 192.168.11.45 \
  --username admin \
  --password secret \
  --moqt-url https://localhost:4433 \
  --insecure-skip-tls-verify \
  --publish-namespace onvif/client \
  --subscribe-namespace onvif/viewer \
  --video-track video \
  --catalog-track catalog \
  --command-track command
```

This publishes the RTSP stream as profile-specific tracks under `onvif/client`, and subscribes to the `command` track
(datagram) under `onvif/viewer`.

Each ONVIF profile becomes a separate track named `<video-track>/profile_N` (for example `video/profile_1`). A catalog
track (`catalog` by default) returns an MSF Catalog JSON object (draft-ietf-moq-msf-00) describing those tracks
(label/codec/width/height). The initial catalog omits codec until SPS/PPS are parsed; it is resent with codec only for
the selected profile once resolved. The bridge waits for the first Subscribe to one of the profile tracks and publishes
only that profile.

## Options

- `--ip`: camera IP address or hostname (required)
- `--username`, `--password`: credentials for ONVIF WS-Security (required)
- `--timeout-ms`: timeout per operation
- `--video-track`: track prefix for video profiles (default `video`, publishes `video/profile_1`, ...)
- `--catalog-track`: track name for the profile catalog (default `catalog`)
- `--payload-format`: send payload as `annexb` or `avcc` (default). AVCC mode converts AnnexB payloads to length-prefixed.
- `--insecure-skip-tls-verify`: skip certificate validation for local debugging only (INSECURE)
- `--dump-keyframe[=PATH]`: dump the first keyframe payload for ffprobe (default path `/tmp/moqt-onvif-keyframe.h264`)

## Implementation

- PTZ GUI layout and command handling live in `moqt-client-onvif/src/ui_ptz.rs`.
- PTZ input defaults and parsing live in `moqt-client-onvif/src/ui_ptz/inputs.rs`.
- MoQ bridge entrypoint is `moqt-client-onvif/src/bin/moqt-onvif-client.rs`.
- RTSP encoded packet capture for MoQ lives in `moqt-client-onvif/src/rtsp_decoder.rs`.
- MoQ send logs include group/object/timestamp for correlating browser-side jitter logs.

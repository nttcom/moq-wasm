# moq-cli

Publishes a raw H.264 stream from stdin to a MoQ relay as an MSF broadcast,
or subscribes to one track and writes it to stdout.

```sh
# local loopback, from an MP4 converted to Annex-B by ffmpeg
ffmpeg -re -i movie.mp4 -c:v copy -bsf:v h264_mp4toannexb -f h264 - \
  | moq-cli publish --relay moqt://localhost:4433 --namespace anon/live --insecure
moq-cli subscribe --relay moqt://localhost:4433 --track anon/live/video --insecure | ffplay -
```

| Flag | publish | subscribe | Meaning |
| --- | :-: | :-: | --- |
| `--relay` | ● | ● | Relay URL, e.g. `moqt://localhost:4433` |
| `--namespace` | ● | | Namespace of the broadcast, e.g. `anon/tokyo/cam01` |
| `--track` | | ● | Full track name, e.g. `anon/tokyo/cam01/video` |
| `--insecure` | ● | ● | Skip certificate verification, for self-signed relays |
| `--auth-token` | ● | ● | JWT for the relay. Also `MOQT_AUTH_TOKEN` |
| `--auth-token-file` | ● | ● | File holding the JWT. Also `MOQT_AUTH_TOKEN_FILE` |
| `--versions` | ● | ● | MoQT drafts offered, most preferred first; `draft-18,draft-14` when omitted. Also `MOQT_VERSIONS` |
| `--telemetry` | ● | ● | Observability server that receives the logs. Also `MOQT_TELEMETRY_URL` |
| `--no-telemetry` | ● | ● | Send no logs to an observability server |

## Publish and subscribe

- `publish` reads an H.264 Annex-B elementary stream and publishes `catalog`,
  `video` (LOC), `video_cmaf` and `video_timeline` through `crates/broadcast`.
- The stream carries no timestamps, so each access unit is stamped with the
  time it was read. Feed a file at its frame rate (`ffmpeg -re`): input that
  arrives faster than it can be sent is dropped.
- `subscribe` writes the payloads of the track to stdout; for `video` that is
  the Annex-B stream.

## Auth token

Without a token the session is anonymous and limited to `anon/`. A file given
with `--auth-token-file` is re-read every 10 s and a changed token is sent to
the relay, so an issuer that rewrites the file before the token expires keeps
the stream running.

```sh
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "<appId>") | .secret' apps.json)" \
  cargo run -q -p auth-token --features cli -- --app-id <appId> --publish site1 --ttl 12h > /etc/moq/token
moq-cli publish --relay moqt://relay:4433 --namespace <appId>/site1 --auth-token-file /etc/moq/token
```

## Telemetry

The CLI posts its `info` and higher logs to an observability server every
10 s (`service.name=moq-cli` in HyperDX). Without `--telemetry`, a local
relay reports to `http://localhost:8095` and any other relay to
`https://observability.moqt.research.skyway.io`.

## Raspberry Pi

Build a static binary on a Mac with cargo-zigbuild and copy it over:

```sh
cargo zigbuild --release --target aarch64-unknown-linux-musl -p moq-cli
scp target/aarch64-unknown-linux-musl/release/moq-cli pi@pi-cam.local:~/

rpicam-vid -t 0 --codec h264 --inline --width 1280 --height 720 --framerate 30 -o - \
  | ./moq-cli publish --relay moqt://<relay>:443 --namespace anon/live
```

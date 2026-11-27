# moq-cli

Publishes raw H.264/H.265 from stdin to a MoQT relay as an MSF broadcast, or subscribes to a track and writes payloads to stdout.

```sh
# Local loopback from an MP4 converted to Annex-B
ffmpeg -re -i movie.mp4 -c:v copy -bsf:v h264_mp4toannexb -f h264 - \
  | moq-cli publish --relay moqt://localhost:4433 --namespace anon/live --insecure
moq-cli subscribe --relay moqt://localhost:4433 --track anon/live/video --insecure | ffplay -
```

| Flag | publish | subscribe | Meaning |
| --- | :-: | :-: | --- |
| `--relay` | ● | ● | Relay URL: `moqt://` (QUIC), `https://` (WebTransport), or `wss://` (WebSocket) |
| `--namespace` | ● | | Broadcast namespace (e.g. `anon/tokyo/cam01`) |
| `--codec` | ● | | Input codec: `h264` (default) or `h265` |
| `--track` | | ● | Full track name (e.g. `anon/tokyo/cam01/video`) |
| `--insecure` | ● | ● | Skip TLS verification (self-signed relays) |
| `--auth-token` | ● | ● | JWT (`MOQT_AUTH_TOKEN`) |
| `--auth-token-file` | ● | ● | JWT file (`MOQT_AUTH_TOKEN_FILE`) |
| `--versions` | ● | ● | MoQT drafts in preference order; default `draft-18,draft-14` (`MOQT_VERSIONS`) |
| `--telemetry` | ● | ● | Observability server URL (`MOQT_TELEMETRY_URL`) |
| `--no-telemetry` | ● | ● | Disable log export |

## Publish and subscribe

- `publish` reads H.264 Annex-B (or H.265 with `--codec h265`, e.g. `ffmpeg ... -c:v copy -f hevc -`) and publishes `catalog`, `video` (LOC), `video_timeline`, and `chat` with the Rust SDK (`sdk/rust`).
- Access units are timestamped on arrival; pace file inputs with `ffmpeg -re` (excess input is dropped).
- `subscribe` writes raw track payloads to stdout (Annex-B for `video`).

## Auth token

Without a token, access is limited to `anon/`. `--auth-token-file` is re-read from 5 minutes before the token expires (every 30 s until it holds a new token), and the new token is pushed to the relay so external rotation keeps the stream alive:

```sh
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "<appId>") | .secret' apps.json)" \
  cargo run -q -p auth-token --features cli -- --app-id <appId> --publish site1 --ttl 12h > /etc/moq/token
moq-cli publish --relay moqt://relay:4433 --namespace <appId>/site1 --auth-token-file /etc/moq/token
```

## Telemetry

Exports `info`+ logs every 10 s (`service.name=moq-cli` in HyperDX). Defaults to `http://localhost:8095` for local relays and `https://observability.moqt.research.skyway.io` otherwise.

## Raspberry Pi

Cross-compile a static binary from macOS with `cargo-zigbuild`:

```sh
cargo zigbuild --release --target aarch64-unknown-linux-musl -p moq-cli
scp target/aarch64-unknown-linux-musl/release/moq-cli pi@pi-cam.local:~/

rpicam-vid -t 0 --codec h264 --inline --width 1280 --height 720 --framerate 30 -o - \
  | ./moq-cli publish --relay moqt://<relay>:443 --namespace anon/live
```

# moq-cli

Publishes a raw H.264 stream from stdin to a MoQ relay as an MSF broadcast,
or subscribes to one track and writes it to stdout.

## Usage

```sh
ffmpeg ... -f h264 - | moq-cli publish --relay <relay> --namespace <namespace>
moq-cli subscribe --relay <relay> --track <namespace>/video | ffplay -
```

| Flag | publish | subscribe | Meaning |
| --- | :-: | :-: | --- |
| `--relay` | ● | ● | Relay URL, e.g. `moqt://localhost:4433` |
| `--namespace` | ● | | Namespace of the broadcast, e.g. `anon/tokyo/cam01` |
| `--track` | | ● | Full track name, e.g. `anon/tokyo/cam01/video` |
| `--insecure` | ● | ● | Skip certificate verification, for self-signed relays |
| `--auth-token` | ● | ● | JWT for the relay. Also `MOQT_AUTH_TOKEN` |
| `--auth-token-file` | ● | ● | File holding the JWT. Also `MOQT_AUTH_TOKEN_FILE`. Exclusive with `--auth-token` |
| `--versions` | ● | ● | MoQT drafts offered, comma-separated and most preferred first, e.g. `draft-18,draft-14`; only `draft-14` when omitted. Also `MOQT_VERSIONS` |
| `--telemetry` | ● | ● | Observability server that receives the logs. Also `MOQT_TELEMETRY_URL` |
| `--no-telemetry` | ● | ● | Send no logs to an observability server |

Input and output are always stdin and stdout.

## Telemetry

The CLI posts its `info` and higher logs to an observability server's
`/api/client-telemetry` every 10 s and once more on exit, where they appear in
HyperDX as `service.name=moq-cli` with `page` set to the subcommand. Without
`--telemetry`, a relay on `localhost`, `127.0.0.1` or `[::1]` reports to
`http://localhost:8095`, and any other relay to
`https://observability.moqt.research.skyway.io`. The relay token, when given,
is sent with the logs so they belong to its app; without one they are `anon`.
A failed report is dropped. Once the server rejects the token (expired), the
following reports go without it, as `anon`, until the token file holds
another token.

## Publish

The input is an H.264 Annex-B elementary stream. `publish` splits it into
access units (`mediapack::h264::ElementaryStreamDemuxer`) and publishes them
through `crates/broadcast`. The namespace carries the same tracks as the
other publishers:

- `catalog`
- `video` (LOC)
- `video_cmaf` (CMAF, sent while the relay subscribes to it)
- `video_timeline`

The stream carries no timestamps; each access unit is stamped with the time
it was read.

Feed a file at its frame rate (`ffmpeg -re`): input that arrives faster than
it can be sent is dropped.

`subscribe` reads the catalog with a joining FETCH and writes the payloads of
the track to stdout; for `video` that is the Annex-B stream.

## Auth token

Without a token the session is anonymous and limited to `anon/`. With
`--auth-token-file`, the file is re-read every 10 s after connecting and a
changed token is sent to the relay to extend the session, so an issuer that
rewrites the file before the token expires (24 h at most by default) keeps
the stream running. moq-cli does not interpret the token.

```sh
AUTH_TOKEN_SECRET="$(jq -r '.[] | select(.appId == "<appId>") | .secret' apps.json)" \
  cargo run -q -p auth-token --features cli -- --app-id <appId> --publish site1 --ttl 12h > /etc/moq/token
moq-cli publish --relay moqt://relay:4433 --namespace <appId>/site1 --auth-token-file /etc/moq/token
```

## Examples

```sh
# local loopback, from an MP4 converted to Annex-B by ffmpeg
ffmpeg -re -i movie.mp4 -c:v copy -bsf:v h264_mp4toannexb -f h264 - \
  | moq-cli publish --relay moqt://localhost:4433 --namespace anon/live --insecure
moq-cli subscribe --relay moqt://localhost:4433 --track anon/live/video --insecure | ffplay -
```

## Raspberry Pi

Build a static binary on a Mac with cargo-zigbuild and copy it over:

```sh
cargo zigbuild --release --target aarch64-unknown-linux-musl -p moq-cli
scp target/aarch64-unknown-linux-musl/release/moq-cli pi@pi-cam.local:~/

rpicam-vid -t 0 --codec h264 --inline --width 1280 --height 720 --framerate 30 -o - \
  | ./moq-cli publish --relay moqt://<relay>:443 --namespace anon/live
```

## Build and test

```sh
cargo build --release -p moq-cli
cargo test -p moq-cli
```

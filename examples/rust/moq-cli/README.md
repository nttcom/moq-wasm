# moq-cli

Publishes an encoded H.264 stream from stdin to a MoQ relay, or subscribes to
one and writes it to stdout.

## Usage

```sh
ffmpeg ... | moq-cli publish --relay <relay> --track <track> --codec <codec>
moq-cli subscribe --relay <relay> --track <track> | ffplay -
```

| Flag | publish | subscribe | Meaning |
| --- | :-: | :-: | --- |
| `--relay` | ● | ● | Relay URL, e.g. `moqt://localhost:4433` |
| `--track` | ● | ● | Full track name, e.g. `anon/tokyo/cam01/video` |
| `--codec` | ● | | Required with `--container loc`: `avc3`. Not used with `cmaf` |
| `--container` | ● | | `loc` (default) or `cmaf` |
| `--insecure` | ● | ● | Skip certificate verification, for self-signed relays |
| `--auth-token` | ● | ● | JWT for the relay. Also `MOQT_AUTH_TOKEN` |
| `--auth-token-file` | ● | ● | File holding the JWT. Also `MOQT_AUTH_TOKEN_FILE`. Exclusive with `--auth-token` |

`subscribe` reads codec and container from the catalog. Timestamps are
stamped from the wall clock. Input and output are always stdin and stdout.

## Container

| `--container` | Input | moq-cli does | Needs codec knowledge |
| --- | --- | --- | --- |
| `loc` (default) | Raw elementary stream (Annex-B) | Splits frames, detects keyframes, one frame per object | yes |
| `cmaf` | CMAF (`moof` + `mdat`, ffmpeg `-f mp4 -movflags frag...`) | One object per box boundary | no |

## Auth token

Without a token the session is anonymous and limited to `anon/`. With
`--auth-token-file`, the file is re-read every 10 s after connecting and a
changed token is sent to the relay to extend the session, so an issuer that
rewrites the file before the token expires (24 h at most by default) keeps
the stream running. moq-cli does not interpret the token.

```sh
node services/vts/bin/mint.mjs --apps services/vts/apps.json --app-id <appId> --publish site1 --ttl 12h > /etc/moq/token
moq-cli publish --relay moqt://relay:4433 --track <appId>/site1/video --codec avc3 --auth-token-file /etc/moq/token
```

## Examples

```sh
# local loopback
cat sample.h264 | moq-cli publish --relay moqt://localhost:4433 --track anon/live/video --codec avc3 --insecure
moq-cli subscribe --relay moqt://localhost:4433 --track anon/live/video --insecure | ffplay -

# from an MP4, converted to Annex-B by ffmpeg
ffmpeg -i movie.mp4 -c:v copy -bsf:v h264_mp4toannexb -f h264 - \
  | moq-cli publish --relay moqt://localhost:4433 --track anon/live/video --codec avc3 --insecure
```

## Raspberry Pi

Build a static binary on a Mac with cargo-zigbuild and copy it over:

```sh
cargo zigbuild --release --target aarch64-unknown-linux-musl -p moq-cli
scp target/aarch64-unknown-linux-musl/release/moq-cli pi@pi-cam.local:~/

rpicam-vid -t 0 --codec h264 --inline --width 1280 --height 720 --framerate 30 -o - \
  | ./moq-cli publish --relay moqt://<relay>:443 --track anon/live/video --codec avc3
```

## Build and test

```sh
cargo build --release -p moq-cli
cargo test -p moq-cli
```

## Known limitation

The catalog is re-sent at every keyframe so that late subscribers receive it.
The intended form is one catalog publish picked up by subscribers with a
joining FETCH.

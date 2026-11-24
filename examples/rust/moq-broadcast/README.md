# moq-broadcast

End-to-end example using only the public API of the Rust SDK [`moq-wasm`](../../../sdk/rust/README.md):

- `publish`: loops an H.264 file as an MSF broadcast with an `events` timeline (playable in [live-viewer](../../browser/examples/live-viewer/README.md)).
- `watch`: follows broadcasts under a prefix, logging catalogs, events, and per-second frame/bitrate stats for the lowest video rendition.

## Run

Start a local relay (`make relay`), then generate an H.264 file without B-frames:

```sh
ffmpeg -f lavfi -i testsrc2=size=640x360:rate=30 -t 10 \
  -c:v libx264 -profile:v baseline -g 30 -bf 0 -f h264 test.h264
```

Run `watch` and `publish` in separate terminals:

```sh
cargo run -p moq-broadcast -- --cert services/relay/keys/cert.pem watch
```

```sh
cargo run -p moq-broadcast -- --cert services/relay/keys/cert.pem publish test.h264
```

Example `watch` output:

```text
anon/live/test: track video Some(Known(Video)) Some("avc1.42C01E") Some(691200) bps
anon/live/test: event {"T":0,"data":{"lap":0}}
anon/live/test: video 30 frames, 901 kbit/s
```

To view in a browser, run `make browser`, open live-viewer, connect to `https://127.0.0.1:4433`, and watch `anon/live/test`.

| Option | Default | Meaning |
| --- | --- | --- |
| `--url` | `moqt://127.0.0.1:4433` | Relay URL (`moqt://` QUIC, `https://` WebTransport) |
| `--cert` / `--insecure` | OS roots | Trust a relay cert file or skip verification |
| `--token` | none | Auth JWT |
| `publish --namespace` | `anon/live/test` | Broadcast namespace |
| `publish --fps` | `30` | Send frame rate |
| `watch --prefix` | `anon/live` | Namespace prefix to follow |

Press Ctrl-C to stop (`publish` withdraws its namespace cleanly).

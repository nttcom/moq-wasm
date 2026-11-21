# moq-broadcast

The Rust SDK [`moq-wasm`](../../../sdk/rust/README.md) end to end, with
nothing but its public API:

- `publish` sends an H.264 file, looping, as the video of an MSF broadcast
  with an `events` event timeline, which the browser's
  [live-viewer](../../browser/examples/live-viewer/README.md) plays.
- `watch` follows every broadcast under a prefix: it prints the catalog,
  each event, and the frames and bitrate of the lowest video rendition every
  second.

## Run

Start a local relay with `make relay` from the repository root, then make an
H.264 file without B-frames, since the file's frames are sent in its order:

```sh
ffmpeg -f lavfi -i testsrc2=size=640x360:rate=30 -t 10 \
  -c:v libx264 -profile:v baseline -g 30 -bf 0 -f h264 test.h264
```

In two terminals:

```sh
cargo run -p moq-broadcast -- --cert services/relay/keys/cert.pem watch
cargo run -p moq-broadcast -- --cert services/relay/keys/cert.pem publish test.h264
```

`watch` prints lines such as:

```text
anon/live/test: track video Some(Known(Video)) Some("avc1.42C01E") Some(691200) bps
anon/live/test: event {"T":0,"data":{"lap":0}}
anon/live/test: video 30 frames, 901 kbit/s
```

To watch it in a browser, run `make browser`, open live-viewer, set the relay
URL to `https://127.0.0.1:4433` and watch `anon/live/test`.

| Option | Default | Meaning |
| --- | --- | --- |
| `--url` | `moqt://127.0.0.1:4433` | The relay: `moqt://` over raw QUIC, `https://` over WebTransport |
| `--cert` / `--insecure` | the system's roots | Trust the relay's certificate file, or any certificate |
| `--token` | none | The JWT of a relay that verifies tokens |
| `publish --namespace` | `anon/live/test` | The broadcast's namespace |
| `publish --fps` | `30` | The frame rate the file is sent at |
| `watch --prefix` | `anon/live` | The namespaces to follow |

Ctrl-C stops either; `publish` withdraws its namespace first.

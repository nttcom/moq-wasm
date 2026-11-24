# recorder

Records MSF broadcasts from a MoQT relay into fragmented MP4 or replayable MoQ archives. `recorder bot` records rooms launched by `bot-dispatcher`; `recorder serve` lists and replays recordings over HTTP. Design: [architecture.md](../../docs/architecture/recorder/architecture.md).

## Record a single broadcast

```bash
cargo run -p recorder -- \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify \
  --namespace anon/live/test
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--relay` (`MOQT_URL`) | — | `moqt://` (QUIC) or `https://` (WebTransport) |
| `--namespace` | — | Broadcast namespace |
| `--list-tracks` | off | Print catalog LOC tracks and exit |
| `--video-track` | by `--max-video-height` | LOC video track (H.264, H.265, VP8, VP9, AV1) |
| `--max-video-height` | `720` | Max height for auto-selected video track |
| `--audio-track` | highest bitrate | LOC audio track (AAC, Opus, G.711 A-law, MP3) |
| `--output` | `<namespace>-<Unix seconds>.mp4` | Output path (never overwrites existing files) |
| `--layout` | `combined` | `combined` (single file) or `per-track` (`<stem>-<track>.mp4`) |
| `--from-cache` | off | Start from relay cache (60 s default), then continue live |
| `--segment-seconds` | — | Roll to a new numbered file at the first keyframe after N seconds |
| `--upload` (`RECORDER_UPLOAD_URL`) | — | Upload closed files to `gs://<bucket>/<prefix>` or `file:///<dir>/` and delete locally |
| `--auth-token` (`MOQT_AUTH_TOKEN`) | — | Static JWT for namespaces outside `anon/**` |
| `--auth-token-file` (`MOQT_AUTH_TOKEN_FILE`) | — | JWT file polled every 10 s; changes are pushed to the relay |
| `--insecure-skip-tls-verify` | off | Skip TLS verification (local dev only) |
| `--archive` | off | Experimental: write JSON Lines instead of MP4 |

- Starts at the next keyframe; stops on Ctrl-C, session close, or 10 s after the publisher ends.
- Flushes fMP4 at every video group (playable even if killed) and preserves capture-timestamp A/V sync.
- Config changes roll to `<stem>-2.mp4`, `<stem>-3.mp4`, … on the same timeline.
- Tracks added in later catalogs join at their next group; ended tracks don't stop other tracks.
- `--upload` uses standard Google environment credentials (like `gcloud`).

## Recording API (`recorder serve`)

`bot-dispatcher` runs room recordings as `recorder bot` jobs (which report `output.recording_id` in heartbeats once the recording exists) and composites as `recorder composite` jobs ([design](../../docs/architecture/bot-dispatcher/architecture.md)). `recorder serve` reads their state from the store (`make recorder-serve` locally):

```bash
cargo run -p recorder -- serve \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify --insecure-skip-auth \
  --store file://$PWD/recordings/store/
```

| Route | Response |
| --- | --- |
| `GET /recordings/{id}` | `state` (`starting`, `recording`, `stopping`, `finished`, `failed`), participant namespaces, and `composite` state |
| `GET /recordings?namespace_prefix=APP/research` | Recordings under that prefix |
| `GET /apps/APP/recordings` | Up to 100 MP4 recordings with file lists, newest first |
| `GET /recordings/{id}/file?name=<name>` | `{"url"}` signed for 15 min (requires a `gs://` store with signing credentials and an existing `.mp4` file; local `file://` stores cannot sign URLs) |
| `POST /recordings/{id}/publish` | Replays a finished MoQ recording to `<room>/recordings/<id>` while the NDJSON stream stays open (driven by `openRecording` in `moq-wasm/recording`) |

- Records every participant namespace under the prefix from the moment it is published (main source and screen share separately) into `<store>/<app>/<recording id>/<namespace>/<track>/`, with `manifest.json` and `composite.mp4` in the recording root.
- Requires `Authorization: Bearer <JWT>` (verified via VTS) whose `subscribe` claim covers the room. `POST /recordings/{id}/publish` reuses the caller's token (renewed via `--temporary-apps-url` for temporary apps) and therefore also requires relay `publish` permission covering `<room>/recordings/<id>`.
- Play replays in `examples/recording-player/`. Compositing requires the GStreamer plugins in [`crates/transcode`](../../crates/transcode/README.md#requirements).

| Option | Default | Meaning |
| --- | --- | --- |
| `--listen` | `127.0.0.1:8096` | HTTP listen address |
| `--allowed-origin` | `http://localhost:5173`, `http://127.0.0.1:5173` | Allowed CORS origins (repeatable) |
| `--auth-vts-url` (`AUTH_VTS_URL`) | — | VTS verify URL (required unless `--insecure-skip-auth`) |
| `--insecure-skip-auth` | off | Skip auth (local dev only) |
| `--temporary-apps-url` (`TEMPORARY_APPS_URL`) | — | Service URL for renewing temporary-app tokens |
| `--max-replays` | `4` | Max concurrent replays |
| `--store` (`RECORDER_STORE_URL`) | — | `gs://<bucket>/<prefix>` or `file:///<dir>/` |
| `--relay`, `--insecure-skip-tls-verify` | | Target relay for replays |

For `recorder bot` flags (`--relay`, `--store`, `--output-dir`, `--segment-seconds`, `--max-video-height`, `--launcher`, `--composite-job`, plus `bot-dispatcher` args), see "Started by `bot-dispatcher`" in the architecture document.

## E2E

```bash
LIVE_VIEWER_E2E_TRANSCODE=0 node tests/browser-e2e/run-live-viewer-e2e.mjs tests/recorder-archive-e2e.spec.ts
```

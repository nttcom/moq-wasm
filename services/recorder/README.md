# recorder

Records an MSF broadcast from a MoQT relay into a fragmented MP4 file: it
subscribes to the catalog, reads one LOC video and one LOC audio track, and
muxes both with `mediapack`. `recorder bot` records whole rooms that `bots`
starts; `recorder serve` lists and replays them over an HTTP API.
Design: [architecture document](../../docs/architecture/recorder/architecture.md).

## Recording one broadcast

```bash
cargo run -p recorder -- \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify \
  --namespace anon/live/test
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--relay` (`MOQT_URL`) | — | `moqt://` for QUIC, `https://` for WebTransport |
| `--namespace` | — | Namespace of the broadcast |
| `--list-tracks` | off | Print the catalog's LOC tracks and exit |
| `--video-track` | by `--max-video-height` | LOC video track to record; H.264, VP8, VP9 or AV1 |
| `--max-video-height` | `720` | Without `--video-track`, the tallest video track at most this high |
| `--audio-track` | the highest bitrate | LOC audio track to record; AAC, Opus, G.711 A-law or MP3 |
| `--output` | `<namespace>-<Unix seconds>.mp4` | Output file; an existing file is never overwritten |
| `--layout` | `combined` | `combined` writes one file; `per-track` writes each track to `<stem>-<track>.mp4` |
| `--from-cache` | off | Start with what the relay still caches (60 s by default), then continue live |
| `--segment-seconds` | — | Start the next numbered file at the first keyframe after this many seconds |
| `--upload` (`RECORDER_UPLOAD_URL`) | — | Upload every closed file to `gs://<bucket>/<prefix>` or `file:///<dir>/` and delete the local file |
| `--auth-token` (`MOQT_AUTH_TOKEN`) | — | JWT for namespaces outside `anon/**`; never refreshed |
| `--auth-token-file` (`MOQT_AUTH_TOKEN_FILE`) | — | File holding the JWT; re-read every 10 s and a changed token is sent to the relay |
| `--insecure-skip-tls-verify` | off | Accept the local relay's development certificate |
| `--archive` | off | Experimental: record the objects as JSON Lines instead of MP4 |

- Recording starts at the next keyframe and ends on Ctrl-C, when the relay
  closes the session, or 10 s after the publisher ends its tracks.
- The file is fragmented MP4 flushed at every video group, so it plays even
  if the recorder is killed.
- Audio and video keep the offset of their capture timestamps, so they stay
  in sync.
- When the video or audio configuration changes, recording continues in
  `<stem>-2.mp4`, `<stem>-3.mp4`, … on the same timeline.
- A track that appears in a later catalog (a participant unmutes or turns
  the camera on) is added from its next group; one that ends leaves the
  other recording alone.
- `--upload` reads Google credentials from the environment, as `gcloud` does.

## Recording API

`bots` starts and stops room recordings ([its design](../../docs/architecture/bots/architecture.md)):
each runs as a `recorder bot` job, and each composite as a `recorder composite`
job. `recorder serve` lists and replays them; their state lives in the store
next to the files. Locally (`make recorder-serve`):

```bash
cargo run -p recorder -- serve \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify --insecure-skip-auth \
  --store file://$PWD/recordings/store/
```

| Request | Answer |
| --- | --- |
| `GET /recordings/{id}` | `state` (`starting`, `recording`, `stopping`, `finished`, `failed`), each participant's namespace, and the `composite` state |
| `GET /recordings?namespace_prefix=APP/research` | The recordings of that prefix |
| `GET /apps/APP/recordings` | The app's MP4 recordings with their files, newest first (at most 100) |
| `GET /recordings/{id}/file?name=<name>` | `{"url"}`: a signed URL that reads the file from the store for 15 minutes |
| `POST /recordings/{id}/publish` | Replays a finished MoQ recording to the relay under `<room>/recordings/<id>` while the NDJSON response stays open; `openRecording` in `moq-wasm/recording` drives it |

- Every namespace under the prefix (one per participant) is recorded from the
  moment it is published: its main source, and the screen share apart from
  it. The files go to `<store>/<app>/<recording id>/<namespace>/<track>/`,
  with `manifest.json` and `composite.mp4` in `<store>/<app>/<recording id>/`.
- Every request needs `Authorization: Bearer <JWT>`, verified by the VTS,
  whose `subscribe` claim covers the room. A replay presents the caller's
  token; with `--temporary-apps-url`, a temporary app's token is renewed
  there.
- A replay is played by `examples/recording-player/` in the browser examples.
- Compositing needs the GStreamer plugins listed in the
  [transcode README](../../crates/transcode/README.md#requirements).

| Option | Default | Meaning |
| --- | --- | --- |
| `--listen` | `127.0.0.1:8096` | Address of the API |
| `--allowed-origin` | `http://localhost:5173`, `http://127.0.0.1:5173` | Pages allowed to call it (CORS); repeat for several |
| `--auth-vts-url` (`AUTH_VTS_URL`) | — | VTS verification endpoint; required unless `--insecure-skip-auth` |
| `--insecure-skip-auth` | off | Accept every request without a token (local development only) |
| `--temporary-apps-url` (`TEMPORARY_APPS_URL`) | — | Temporary-apps service renewing temporary apps' tokens |
| `--max-replays` | `4` | Recordings replayed at once |
| `--store` (`RECORDER_STORE_URL`) | — | `gs://<bucket>/<prefix>` or `file:///<dir>/`: the files and the recordings' state |
| `--relay`, `--insecure-skip-tls-verify` | | The relay a replay publishes through |

`recorder bot` takes the job's settings (`--relay`, `--store`, `--output-dir`,
`--segment-seconds`, `--max-video-height`, `--launcher` and `--composite-job`
for the composite) and the ones `bots` adds; see the architecture document,
"Started by `bots`".

## E2E

```bash
LIVE_VIEWER_E2E_TRANSCODE=0 node tests/browser-e2e/run-live-viewer-e2e.mjs tests/recorder-archive-e2e.spec.ts
```

# recorder

Records an MSF broadcast from a MoQT relay into a fragmented MP4 file. It
subscribes to the catalog, reads one LOC video and one LOC audio track that
`crates/broadcast` publishers (`live-ingest`, `moqtsink`, `onvif-ingest`)
and the WebCodecs publishers in `examples/browser` produce, and muxes both
into one file with `mediapack`.

```bash
cargo run -p recorder -- \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify \
  --namespace anon/live/test
```

For development, `--archive --output recording.jsonl` instead records the
catalog and selected LOC audio and video objects as JSON Lines. This initial
format is experimental: it has no seek index and is not a complete MoQ
archive. Catalog and media tracks retain object status, datagrams and
subgroup headers. With `--upload`, it also writes checksummed data and index
chunks to the object store, then commits their manifest. It does not support
cached history or timed segmentation. A final `complete` line marks a normal
stop; an interrupted write or relay failure leaves that line absent.
The browser's `examples/archive-player` is an experimental player for
single-broadcast archives. It indexes the file on open and reads the media
in keyframe-aligned windows during playback. Its end-to-end test is:

```bash
LIVE_VIEWER_E2E_TRANSCODE=0 node tests/browser-e2e/run-live-viewer-e2e.mjs tests/archive-player-e2e.spec.ts
```

To see which tracks a broadcast offers, print its catalog:

```bash
cargo run -p recorder -- \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify \
  --namespace anon/research/alice --list-tracks
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--relay` (`MOQT_URL`) | — | `moqt://` for QUIC, `https://` for WebTransport |
| `--namespace` | — | Namespace of the broadcast |
| `--video-track` | by `--max-video-height` | LOC video track to record (`video_720p`, `video_480p`, ...); H.264, VP8, VP9 or AV1, as the catalog's `codec` names it |
| `--max-video-height` | `720` | Without `--video-track`, the main source's tallest video track at most this high, or the shortest one when all are taller |
| `--audio-track` | the main source's highest bitrate | LOC audio track to record; AAC, Opus, G.711 A-law and MP3 |
| `--list-tracks` | off | Print the catalog's LOC tracks (name, role, codec, size, bitrate, sample rate) and exit |
| `--output` | `<namespace>-<Unix seconds>.mp4` | Output file; an existing file is never overwritten |
| `--layout` | `combined` | `combined` writes the video and the audio into one file; `per-track` writes each track to `<stem>-<track>.mp4` (a `/` in the name becomes `_`) |
| `--from-cache` | off | Start with what the relay or the publisher still caches (60 s by default), then continue live |
| `--segment-seconds` | — | Start the next numbered file at the first keyframe after this many seconds |
| `--upload` (`RECORDER_UPLOAD_URL`) | — | Upload every closed file to `gs://<bucket>/<prefix>` (or a `file:///<dir>/`) and delete the local file once it is uploaded |
| `--auth-token` (`MOQT_AUTH_TOKEN`) | — | JWT for namespaces outside `anon/**` |
| `--auth-token-file` (`MOQT_AUTH_TOKEN_FILE`) | — | File holding the JWT; re-read every 10 s and a changed token is sent to the relay |
| `--insecure-skip-tls-verify` | off | Accept the local relay's development certificate |

## Recording API

`recorder serve` records whole rooms on request, for the meeting example. It
keeps no recording itself: each recording runs as `recorder record`, each
composite as `recorder composite`, and their state lives in the store next to
the files. Locally (`make recorder-serve`):

```bash
cargo run -p recorder -- serve \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify --insecure-skip-auth \
  --store file://$PWD/recordings/store/ --output-dir recordings/work --composite
```

| Request | Answer |
| --- | --- |
| `POST /recordings` `{"namespace_prefix": "APP/research", "layout": "per-track", "composite": true}` | `201` with the recording, `starting` until its job has joined the room; `layout` is `combined` by default. `composite: true` also makes `composite.mp4`, every participant in one grid with mixed audio, once the recording has ended; it needs `--composite` (`400` otherwise). `400` for a prefix that is not `<app>/<room>`, `403` from a page outside `--allowed-origin`, `409` when the prefix is already recorded, `503` past `--max-recordings`, `500` when the store or the launcher fails |
| `POST /recordings/{id}/stop` | `202`; the state turns `stopping` within 5 s, then `finished` once every file is closed and uploaded |
| `GET /recordings/{id}` | `state` (`starting`, `recording`, `stopping`, `finished`, `failed`; a job silent for 90 s is reported `failed`), each participant's namespace, start time, `finished` and `error`, and `composite` (`null`, or `state` `waiting`, `composing`, `done`, `skipped`, `failed` with `error`) |
| `GET /recordings?namespace_prefix=APP/research` | The recordings of that prefix (`400` without it) |

Every namespace under the prefix (one per meeting participant) is recorded
from the moment it is published, with the tracks picked from its catalog
(`--max-video-height`, 720 by default): the main source, the camera and
the microphone, and apart from it the screen share (see Behaviour). The
objects go to `<store>/<app>/<recording id>/<namespace>/<track>/`, `<app>`
being the room's first element, with `manifest.json` and `composite.mp4` in
`<store>/<app>/<recording id>/`; compositing
needs the GStreamer plugins listed in the
[transcode README](../../crates/transcode/README.md#compositing).

An experimental `"format": "moq"` records participant camera, audio,
screen-share and chat tracks with the media timelines of the camera and the
screen share, and the meeting transcriber's transcript, as object-store chunks
instead of MP4. Omitting `format` retains MP4 behavior.

To replay a finished MoQ recording, send `POST /recordings/<id>/publish`
(`409` for an MP4 recording). The response is an NDJSON stream that stays open.
Its first line is
`{"namespace_prefix": "<room>/recordings/<id>", "renew_after_seconds": 2700,
"started_at_ms": <ms>}`; empty keepalive lines follow. The replay's LOC Timestamps are media time since
`started_at_ms`, the Unix time in milliseconds the recording started. Under the prefix, each participant's camera and
screen share is an MSF VOD broadcast,
`<prefix>/<participant>/camera/<n>` and `<prefix>/<participant>/screen_share/<n>`,
where `<participant>` is the namespace's elements after the room and `<n>`
counts the times the participant joined, from 0. The transcriber's namespace is
not replayed: a participant's camera catalog lists their part of the transcript
as the `transcript` event timeline. Find the namespaces with
SUBSCRIBE_NAMESPACE, read each `catalog` with SUBSCRIBE and a joining FETCH,
which lists the recorded tracks with `isLive: false` and `trackDuration`, place
the groups with the media timeline it lists, which covers the whole recording,
and FETCH the media. The recorder reads and checks the stored chunks without
generating an MP4. The namespace stays available while at least one response
is open and closes when the last one closes. Open the next response after
`renew_after_seconds`, before Cloud Run's request timeout ends the current one,
and close the previous one a minute later, once FETCHes sent to it have ended;
`openRecording` in `moq-wasm/recording` does both. Replay
publications belong to one API process; keep that instance running while
serving them.

`examples/recording-player/` in the browser examples plays this replay, every
participant in a `LivePlayer` of one `PlaybackGroup`, with the chat and the
transcript beside them. The meeting's Activities menu offers **Record meeting
with chat and transcript**.

Every request needs `Authorization: Bearer <JWT>`, verified at
`--auth-vts-url <VTS verify endpoint>` (`AUTH_VTS_URL`): `subscribe` grants
viewing/replay, `publish` grants starting/stopping, each covering the source
room. There is no public room: the anonymous app's rooms, which carry no
token, are not recorded. Tokens do not enter the archive.

A room is recorded and replayed with the caller's bearer token: the recording
job gets it as `--token`, and the relay ends the session when it expires, which
ends the recording. With `--token-mint-url` (`TOKEN_MINT_URL`), the token of a
temporary app (`temp-…` root, see `services/token-mint`) is renewed at the
token mint five minutes before it expires instead, each new one sent to the
relay in-session, so such a recording outlives the page that started it. The
token passes through the job's arguments, so whoever can read the launched
job's configuration can read it.

`--insecure-skip-auth` accepts every request without a token and records with
the token a request carries, or none, for a local relay without authorization.

```bash
LIVE_VIEWER_E2E_TRANSCODE=0 node tests/browser-e2e/run-live-viewer-e2e.mjs tests/recorder-archive-e2e.spec.ts
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--listen` | `127.0.0.1:8096` | Address of the API |
| `--allowed-origin` | `http://localhost:5173`, `http://127.0.0.1:5173` | Pages allowed to call it (CORS); repeat for several |
| `--auth-vts-url` (`AUTH_VTS_URL`) | — | VTS verification endpoint for the requests' tokens; required unless `--insecure-skip-auth` |
| `--insecure-skip-auth` | off | Accept every request without a token (local development only) |
| `--token-mint-url` (`TOKEN_MINT_URL`) | — | Token mint renewing temporary apps' tokens; without it a temporary app's recording ends when its token expires |
| `--max-recordings` | `4` | Rooms recorded at once |
| `--store` (`RECORDER_STORE_URL`) | — | `gs://<bucket>/<prefix>` or `file:///<dir>/`: the files and the recordings' state; a `gs://` store needs the metadata server of Google Cloud to release a room |
| `--output-dir` | `recordings` | Where a recording job writes its files until they are uploaded, and a composite job downloads them |
| `--composite` | off | Accept `composite: true` |
| `--launcher` | `local` | `local` runs the jobs as child processes; `cloud-run` starts executions of `--record-job` and `--composite-job` (`RECORDER_RECORD_JOB`, `RECORDER_COMPOSITE_JOB`, as `projects/<p>/locations/<r>/jobs/<j>`) with the service account of the instance |
| `--max-recording-seconds` | `7200` | A recording stops by itself after this long |
| `--empty-room-seconds` | `300` | A recording stops by itself once nobody has been recorded for this long, from the start too |
| `--segment-seconds`, `--max-video-height`, `--insecure-skip-tls-verify` | | As for a single recording |

`recorder record --id <id> --store <url>` and `recorder composite --id <id>
--store <url>` are the jobs; `recorder serve` passes them everything else.
On Cloud Run the record job needs about 1 vCPU, the composite job as much
CPU as the composite should take, and both a writable `--output-dir`.

## Behaviour

- A source is the LOC tracks of one catalog `renderGroup`. The recorder
  records the main source, the one holding a track the chat (an event
  timeline) lists in `depends`, or the first source of a catalog without an
  event timeline. A room recording records the first other source, the
  screen share, apart from it. Track names are not read.
- Without `--video-track`, the recorder takes the source's LOC track with
  role `video` and a codec it can record whose `height` is the largest at or
  below `--max-video-height` (the smallest when every track is taller), and
  of those the lowest `bitrate`; without `--audio-track`, the source's LOC
  track with role `audio` and the highest `bitrate` among those it can
  decode. A remaining tie goes to the track listed first. A meeting
  participant encodes a rendition only while someone subscribes to it, the
  recorder included. The choice is made once, from the catalog at start,
  and logged as `catalog resolved`; only a track missing at start, or one
  that ended, is added later (see below).
- Recording starts at the next group, i.e. the next keyframe, or with
  `--from-cache` at the oldest keyframe still cached, and ends on
  Ctrl-C, when the relay closes the session, or 10 s after the publisher ends
  the catalog and the media tracks (PUBLISH_DONE), the time `moqt` waits for
  late streams.
- The file is fragmented MP4, so it plays even if the recorder is killed; the
  file is flushed to disk whenever a video group completes, or an audio group
  while no video is recorded.
- Audio and video keep the offset of their capture timestamps, so the two
  tracks stay in sync however the subscriptions started, also across the two
  files of `--layout per-track`.
- When the video or audio configuration changes (resolution, codec profile),
  or with `--segment-seconds` when a segment reached its length, the current
  file is closed and recording continues in `<stem>-2.mp4`, `<stem>-3.mp4`,
  ... (`<stem>-<track>-2.mp4` per track). Every file starts with its own init
  segment and a keyframe (any audio frame in an audio-only file), and keeps
  the recording's timeline, so the second file of 60 s segments starts at 60 s.
- While the catalog lists no audio track the recorder can decode (a
  `broadcast` publisher lists AAC before its first audio frame delivers the
  configuration; a meeting participant lists audio only while the microphone
  is on), the video is recorded alone and each later catalog is checked. The
  first audio track one lists is recorded from its next group: in the
  combined file at once if no video has been written yet, otherwise from the
  next keyframe on in `<stem>-2.mp4`; per track in its own file. A codec
  without an fMP4 mapping is never recorded.
- When the catalog no longer lists the audio track, or the track ends (a
  meeting participant mutes), the video goes on alone and the audio is
  picked again from a later catalog (the participant unmutes). It continues
  in the file the audio went to, after a gap, while its configuration is the
  same; otherwise it starts the next file as above.
- The video does the same (a meeting participant turns the camera off and
  on): the audio goes on alone meanwhile, and a recording whose first
  catalog lists no video starts with the audio. A video track listed later
  is recorded from its next group, in the file the video went to while its
  configuration is the same (the last frame before the gap stays on
  screen); otherwise, and in a combined file without video, from its
  keyframe on in the next file.
- With `--upload`, each closed file becomes the object
  `<prefix>/<namespace>/<track>/<start Unix ms>-<index>.mp4` (no `<track>` for
  the combined file); `file:///<dir>` writes the same keys into a directory.
  The capture origin is only logged (`recorded file closed`); a room
  recording lists it per file in its manifest. The credentials come from the
  environment, as for `gcloud`:
  `GOOGLE_APPLICATION_CREDENTIALS` or `GOOGLE_SERVICE_ACCOUNT`, the ADC file
  that `gcloud auth application-default login` writes, or the metadata server
  of a GCE VM or Cloud Run. A file whose upload fails stays on disk. A file
  is uploaded when it closes, so without `--segment-seconds` a recording
  reaches the store only when it ends.
- A token given with `--auth-token` is never refreshed, so the relay ends a
  recording that outlives it. With `--auth-token-file`, rewrite the file
  before the token expires and the recorder sends the new token in-session.

The reasoning behind these choices is in the
[architecture document](../../docs/architecture/recorder/architecture.md).

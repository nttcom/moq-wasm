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
| `--video-track` | by `--max-video-height` | LOC video track to record (`video_720p`, `camera_main_720p`, ...); H.264, VP8, VP9 or AV1, as the catalog's `codec` names it |
| `--max-video-height` | `480` | Without `--video-track`, the tallest video track at most this high, or the shortest one when all are taller |
| `--audio-track` | highest bitrate | LOC audio track to record; AAC, Opus, G.711 A-law and MP3 |
| `--list-tracks` | off | Print the catalog's LOC tracks (name, role, codec, size, bitrate, sample rate) and exit |
| `--output` | `<namespace>-<Unix seconds>.mp4` | Output file; an existing file is never overwritten |
| `--layout` | `combined` | `combined` writes the video and the audio into one file; `per-track` writes each track to `<stem>-<track>.mp4` (a `/` in the name becomes `_`) |
| `--from-cache` | off | Start with what the relay or the publisher still caches (60 s by default), then continue live |
| `--segment-seconds` | — (60 with `--upload`) | Start the next numbered file at the first keyframe after this many seconds |
| `--upload` (`RECORDER_UPLOAD_URL`) | — | Upload every closed file to `gs://<bucket>/<prefix>` and delete the local file once it is uploaded |
| `--auth-token` (`MOQT_AUTH_TOKEN`) | — | JWT for namespaces outside `anon/**` |
| `--auth-token-file` (`MOQT_AUTH_TOKEN_FILE`) | — | File holding the JWT; re-read every 10 s and a changed token is sent to the relay |
| `--insecure-skip-tls-verify` | off | Accept the local relay's development certificate |

## Recording API

`recorder serve` records whole rooms on request, for the meeting example:

```bash
cargo run -p recorder -- serve \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify \
  --upload gs://moq-recorder/meetings
```

| Request | Answer |
| --- | --- |
| `POST /recordings` `{"namespace_prefix": "anon/research", "layout": "per-track"}` | `201` with the recording; `layout` is `combined` by default. `403` outside `--namespace-root` or from a page outside `--allowed-origin`, `409` when the prefix is already recorded, `503` past `--max-recordings` |
| `POST /recordings/{id}/stop` | `202`; the state turns `stopping`, then `finished` once every file is closed and uploaded |
| `GET /recordings/{id}` | `state` (`recording`, `stopping`, `finished`, `failed`) and each participant's namespace, start time, `finished` and `error` |
| `GET /recordings?namespace_prefix=anon/research` | The recordings of that prefix |

Every namespace under the prefix (one per meeting participant) is recorded
from the moment it is published, with the tracks picked from its catalog
(`--max-video-height`, 480 by default). Files go to
`--output-dir/<recording id>/`, objects to
`<upload prefix>/<recording id>/<namespace>/<track>/`.

| Option | Default | Meaning |
| --- | --- | --- |
| `--listen` | `127.0.0.1:8096` | Address of the API |
| `--allowed-origin` | `http://localhost:5173`, `http://127.0.0.1:5173` | Pages allowed to call it (CORS); repeat for several |
| `--namespace-root` | `anon` | First element every recorded prefix must have; the API has no authentication |
| `--max-recordings` | `4` | Recordings at once |
| `--max-recording-seconds` | `7200` | A recording stops by itself after this long |
| `--empty-room-seconds` | `300` | A recording stops by itself once nobody has been recorded for this long, from the start too |
| `--output-dir` | `recordings` | Where the files are written |
| `--upload`, `--segment-seconds`, `--max-video-height`, `--insecure-skip-tls-verify` | | As for a single recording |

## Behaviour

- Without `--video-track`, the recorder takes the catalog's LOC track with
  role `video` and a codec it can record whose `height` is the largest at or
  below `--max-video-height` (the smallest when every track is taller), and
  of those the lowest `bitrate`; without `--audio-track`, the LOC track with
  role `audio` and the highest `bitrate` among those it can decode. A
  remaining tie goes to the track listed first. A light default matters
  because a meeting participant encodes a rendition only while someone
  subscribes to it, the recorder included. The choice is made once, from
  the catalog at start, and logged as `catalog resolved`; only an audio
  track missing at start, or one that ended, is added later (see below).
- Recording starts at the next group, i.e. the next keyframe, or with
  `--from-cache` at the oldest keyframe still cached, and ends on
  Ctrl-C, when the relay closes the session, or 10 s after the publisher ends
  the video track (PUBLISH_DONE), the time `moqt` waits for late streams.
- The file is fragmented MP4, so it plays even if the recorder is killed; the
  file is flushed to disk whenever a video group completes.
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
- With `--upload`, each closed file becomes the object
  `<prefix>/<namespace>/<track>/<start Unix ms>-<index>.mp4` (no `<track>` for
  the combined file), with the capture origin in its `capture-origin-us`
  metadata. The credentials come from the environment, as for `gcloud`:
  `GOOGLE_APPLICATION_CREDENTIALS` or `GOOGLE_SERVICE_ACCOUNT`, the ADC file
  that `gcloud auth application-default login` writes, or the metadata server
  of a GCE VM or Cloud Run. A file whose upload fails stays on disk.
- A token given with `--auth-token` is never refreshed, so the relay ends a
  recording that outlives it. With `--auth-token-file`, rewrite the file
  before the token expires and the recorder sends the new token in-session.

The reasoning behind these choices is in the
[architecture document](../../docs/architecture/recorder/architecture.md).

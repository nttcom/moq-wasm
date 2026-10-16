# recorder

Records an MSF broadcast from a MoQT relay into a fragmented MP4 file. It
subscribes to the catalog, reads the LOC `video` and `audio` tracks that
`crates/broadcast` publishers (`live-ingest`, `moqtsink`, `onvif-ingest`)
and the WebCodecs publishers in `examples/browser` produce, and muxes both
into one file with `mediapack`.

```bash
cargo run -p recorder -- \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify \
  --namespace anon/live/test
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--relay` (`MOQT_URL`) | — | `moqt://` for QUIC, `https://` for WebTransport |
| `--namespace` | — | Namespace of the broadcast |
| `--video-track` | `video` | LOC video track to record (`video_720p`, `video/profile_1`, ...); H.264, VP8, VP9 or AV1, as the catalog's `codec` names it |
| `--audio-track` | `audio` | LOC audio track to record; AAC, Opus, G.711 A-law and MP3 |
| `--output` | `<namespace>-<Unix seconds>.mp4` | Output file; an existing file is never overwritten |
| `--layout` | `combined` | `combined` writes the video and the audio into one file; `per-track` writes each track to `<stem>-<track>.mp4` (a `/` in the name becomes `_`) |
| `--from-cache` | off | Start with what the relay or the publisher still caches (60 s by default), then continue live |
| `--segment-seconds` | — | Start the next numbered file at the first keyframe after this many seconds |
| `--auth-token` (`MOQT_AUTH_TOKEN`) | — | JWT for namespaces outside `anon/**` |
| `--auth-token-file` (`MOQT_AUTH_TOKEN_FILE`) | — | File holding the JWT; re-read every 10 s and a changed token is sent to the relay |
| `--insecure-skip-tls-verify` | off | Accept the local relay's development certificate |

## Behaviour

- Recording starts at the next group, i.e. the next keyframe, or with
  `--from-cache` at the oldest keyframe still cached, and ends on
  Ctrl-C, when the relay closes the session, or 10 s after the publisher ends
  a recorded track (PUBLISH_DONE), the time `moqt` waits for late streams.
- The file is fragmented MP4, so it plays even if the recorder is killed; the
  file is flushed to disk whenever a video group completes.
- Audio and video keep the offset of their capture timestamps, so the two
  tracks stay in sync however the subscriptions started, also across the two
  files of `--layout per-track`.
- When the video or audio configuration changes (resolution, codec profile),
  or with `--segment-seconds` when a segment reached its length, the current
  file is closed and recording continues in `<stem>-2.mp4`, `<stem>-3.mp4`,
  ... (`<stem>-<track>-2.mp4` per track). Every file starts with its own init
  segment and a keyframe (any frame for audio alone), and keeps
  the recording's timeline, so the second file of 60 s segments starts at 60 s.
- An audio track the catalog lists without its configuration (a `broadcast`
  publisher that has not seen an audio frame yet, or a codec without an fMP4
  mapping) is left out with a warning and the video is recorded alone.
- A token given with `--auth-token` is never refreshed, so the relay ends a
  recording that outlives it. With `--auth-token-file`, rewrite the file
  before the token expires and the recorder sends the new token in-session.

The reasoning behind these choices is in the
[architecture document](../../docs/architecture/recorder/architecture.md).

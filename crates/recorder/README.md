# recorder

Records an MSF broadcast from a MoQT relay into a fragmented MP4 file. It
subscribes to the catalog, reads the LOC `video` and `audio` tracks that
`crates/broadcast` publishers (`live-ingest`, `moqtsink`, `onvif-ingest`)
produce, and muxes both into one file with `mediapack`.

```bash
cargo run -p recorder -- \
  --relay https://127.0.0.1:4433 --insecure-skip-tls-verify \
  --namespace anon/live/test
```

| Option | Default | Meaning |
| --- | --- | --- |
| `--relay` (`MOQT_URL`) | — | `moqt://` for QUIC, `https://` for WebTransport |
| `--namespace` | — | Namespace of the broadcast |
| `--video-track` | `video` | LOC video track to record (`video_720p`, `video/profile_1`, ...) |
| `--audio-track` | `audio` | LOC audio track to record; AAC, Opus and G.711 A-law |
| `--output` | `<namespace>-<Unix seconds>.mp4` | Output file; an existing file is never overwritten |
| `--auth-token` (`MOQT_AUTH_TOKEN`) | — | JWT for namespaces outside `anon/**` |
| `--insecure-skip-tls-verify` | off | Accept the local relay's development certificate |

## Behaviour

- Recording starts at the next group, i.e. the next keyframe, and ends on
  Ctrl-C, when the relay closes the session, or 10 s after the publisher ends
  a recorded track (PUBLISH_DONE), the time `moqt` waits for late streams.
- The file is fragmented MP4, so it plays even if the recorder is killed; the
  file is flushed to disk whenever a video group completes.
- Audio and video keep the offset of their capture timestamps, so the two
  tracks stay in sync however the subscriptions started.
- When the video or audio configuration changes (resolution, codec profile),
  the current file is closed and recording continues in `<stem>-2.mp4`,
  `<stem>-3.mp4`, ...
- An audio track the catalog lists without its configuration (a `broadcast`
  publisher that has not seen an audio frame yet, or a codec without an fMP4
  mapping) is left out with a warning and the video is recorded alone.
- The token is not refreshed; a recording longer than the token's lifetime
  ends when the relay closes the session.

The reasoning behind these choices is in the
[architecture document](../../docs/architecture/recorder/architecture.md).

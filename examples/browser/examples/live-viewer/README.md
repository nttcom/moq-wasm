# live-viewer

Plays a stream that `services/live-ingest`, the GStreamer `moqtsink` or this
page's MP4 publisher sends to a relay, switches between the renditions the
catalog lists, and rewinds through what the relay still caches.
Design: [Live Player architecture](../../../../docs/architecture/browser-examples/live-player.md).

## Start a stream

Every route publishes `anon/live/test`; enter that namespace in the viewer
and press Watch. OBS and ffmpeg sources need H.264 video and AAC audio.

| Route                                         | Commands, each in its own terminal                                                                               | Relay URL in the viewer                       |
| --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| MP4 file from this page                       | Choose a file in MP4 Publish and press Publish                                                                   | The one already set                           |
| ffmpeg → cloud ingest → cloud relay           | `make ffmpeg-srt-bbb-remote`                                                                                     | `https://relay-1.moqt.research.skyway.io:443` |
| ffmpeg → local GStreamer ingest → cloud relay | `make gst-srt-publish GST_MOQT_URL=https://relay-1.moqt.research.skyway.io:443`, `make ffmpeg-srt-bbb-local`     | `https://relay-1.moqt.research.skyway.io:443` |
| ffmpeg → local ingest → local relay           | `make relay`, `make gst-srt-publish` (or `make live-ingest`), `make ffmpeg-srt-bbb-local` (or `make ffmpeg-srt`) | `https://127.0.0.1:4433`                      |

Instead of ffmpeg, point OBS at
`srt://<ingest host>:9000?mode=caller&streamid=anon/live/test`, with
`relay-1.moqt.research.skyway.io` or `localhost` as the ingest host (RTMP:
server `rtmp://<ingest host>:1935/anon/live/test`, key `stream`).

The viewer's relay URL defaults to the cloud `relay-1`; `?moqtUrl=...`
overrides it.

## Controls

The controls appear while the pointer is over the picture.

| Control                                       | Does                                                                                                                              |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `↺5` `↺1` `1↻` `5↻`, ← / → (1 s), ↓ / ↑ (5 s) | Skip through the relay cache                                                                                                      |
| Seek bar                                      | Spans the broadcast; the thin bar underneath is the part the relay still caches (60 s by default), the only part a seek can reach |
| Centre button                                 | Pause and resume                                                                                                                  |
| `LIVE`                                        | Back to live                                                                                                                      |
| Speed                                         | 0.5x to 2x, while rewinding in CMAF mode                                                                                          |
| Gear                                          | Video rendition, audio track and packaging: `LOC` (WebCodecs) or `CMAF` (MediaSource)                                             |
| `Min buffer` / `Max buffer`, `Catch up`       | Bounds of the adaptive jitter buffer and how excess latency is shed: `Skip`, `Speed up` or `Off`                                  |

Rewinding plays from the relay cache through FETCH and cannot go further back
than the relay keeps (`RELAY_CACHE_TTL_SECS`, 60 s).

## MP4 Publish

Publishes an MP4 from the browser as the same catalog and LOC tracks
`live-ingest` produces, on its own MoQT session.

- Video must be H.264; audio may be AAC or MP3, and other audio is skipped.
- Loop restarts the file at its end.
- The preview shows the picture as published, with the viewer's lag below it
  as `pub to sub delay`.

## E2E

With the relay, a source and the dev server running (MP4 publishing needs no
source):

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=live \
npm --prefix examples/browser run e2e:live-viewer     # or e2e:live-viewer-mp4
```

The delivery check compares what the viewer received over `DELIVERY_SECONDS`
(default 30) with the bridge's delivery log and fails when a published sample
is missing:

```shell
RUST_LOG=info,broadcast::delivery=debug make live-ingest 2>&1 | tee /tmp/live-ingest.log
DELIVERY_BRIDGE_LOG=/tmp/live-ingest.log \
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=anon/live/test \
npm --prefix examples/browser run e2e:live-viewer-delivery
```

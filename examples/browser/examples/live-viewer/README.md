# live-viewer

Plays a stream that `bridges/live-ingest`, the GStreamer `moqtsink` or this
page's MP4 publisher sends to a relay, switches between the renditions the
catalog lists, and rewinds through what the relay still caches.

## Start a stream

Pick one route and run each command in its own terminal. OBS and ffmpeg
sources need H.264 video and AAC audio. Every route publishes
`anon/live/test`; enter that namespace in the viewer and press Watch.

### MP4 file from this page → cloud relay

In MP4 Publish choose an MP4 with H.264 video and AAC or MP3 audio and press
Publish. The Stream relay URL and namespace are used; keep them and press Watch.

### OBS / ffmpeg → cloud ingest → cloud relay

```shell
make ffmpeg-srt-bbb-remote
```

Or point OBS at `srt://relay-1.moqt.research.skyway.io:9000?mode=caller&streamid=anon/live/test`
(RTMP: server `rtmp://relay-1.moqt.research.skyway.io:1935/anon/live/test`, key `stream`).
Relay URL in the viewer: `https://relay-1.moqt.research.skyway.io:443`.

### OBS / ffmpeg → local GStreamer ingest → cloud relay

```shell
make gst-srt-publish GST_MOQT_URL=https://relay-1.moqt.research.skyway.io:443
make ffmpeg-srt-bbb-local        # or make ffmpeg-srt for a test pattern
```

Or point OBS at `srt://localhost:9000?mode=caller&streamid=anon/live/test`.
Relay URL in the viewer: `https://relay-1.moqt.research.skyway.io:443`.

### OBS / ffmpeg → local ingest → local relay

```shell
make relay
make gst-srt-publish             # or make live-ingest
make ffmpeg-srt-bbb-local        # or make ffmpeg-srt, or OBS as above
```

Relay URL in the viewer: `https://127.0.0.1:4433`.

## Controls

The controls appear while the pointer is over the picture.

| Control                                       | Does                                                                                                                                                                              |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `↺5` `↺1` `1↻` `5↻`, ← / → (1 s), ↓ / ↑ (5 s) | Skip through the relay cache                                                                                                                                                      |
| Seek bar                                      | Spans the broadcast. The thin bar underneath is the part the relay still caches (60 s by default) and the only part a seek can reach. Home jumps to its oldest point, End to live |
| `position / broadcast`                        | Elapsed time from the publisher's media timeline                                                                                                                                  |
| Centre button                                 | Pause and resume. Resuming live playback returns to the live edge                                                                                                                 |
| `LIVE`                                        | Back to live. Red dot while live, filled while behind                                                                                                                             |
| Speed                                         | 0.5x to 2x, available while rewinding in CMAF mode                                                                                                                                |
| Volume, fullscreen                            | Live audio gain; fullscreen with auto-hiding controls                                                                                                                             |
| Gear                                          | Video rendition, audio track and packaging: `LOC` (WebCodecs) or `CMAF` (MediaSource)                                                                                             |
| `Min buffer` / `Max buffer`, `Catch up`       | Bounds of the adaptive jitter buffer (200 ms minimum by default) and how excess latency is shed: `Skip`, `Speed up` or `Off`                                                      |

Rewinding plays from the relay cache through FETCH and keeps running behind
live until `LIVE` is pressed. It cannot go further back than the relay keeps
(`RELAY_CACHE_TTL_SECS`, 60 s); groups the relay never cached are fetched from
the publisher when it has them, which a browser publisher has not.

A spinner appears when the picture has stood still for half a second. The
stats line reports buffer and target, shed latency, `A/V` offset, audio breaks
and dropped or late video frames.

## MP4 Publish

Publishes an MP4 from the browser as the same catalog and LOC tracks
`live-ingest` produces, so the viewer and the rewind work unchanged.

- Video must be H.264. Audio may be AAC or MP3; other audio codecs are skipped and only the video is published.
- Loop restarts the file at its end; without it publishing stops there.
- The preview on the left shows the picture a receiver without network delay would see, with the viewer's lag below it as `viewer delay`. Publish Streams on the right lists the streams sent per track.
- Publishing uses its own MoQT session, so Watch and Stop do not affect it.

## Relay URL

Defaults to the cloud `relay-1`. The presets cover the cloud load balancer,
`relay-1` to `relay-3` and the local `relay-a` / `relay-b`; `?moqtUrl=...`
overrides the default.

## E2E

With relay, `live-ingest`, a source and the dev server running:

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=live \
npm --prefix examples/browser run e2e:live-viewer
```

MP4 publishing needs only the relay and the dev server:

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
npm --prefix examples/browser run e2e:live-viewer-mp4
```

The delivery check watches a stream for `DELIVERY_SECONDS` (default 30) and
compares what the viewer received with the bridge's delivery log; it fails
when a published sample is missing:

```shell
RUST_LOG=info,media_publisher::delivery=debug make live-ingest 2>&1 | tee /tmp/live-ingest.log
DELIVERY_BRIDGE_LOG=/tmp/live-ingest.log \
DELIVERY_SECONDS=60 \
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=anon/live/test \
npm --prefix examples/browser run e2e:live-viewer-delivery
```

## Design

How the catalog, rewind, A/V synchronisation, packaging and the MP4 publisher
work: [Live Player architecture](../../../../architecture_decision_record/browser-examples/live-player.md).

# live-viewer

Plays MSF broadcasts from `services/ingest-gateway`, GStreamer `moqtsink`, or the page's built-in MP4 publisher, with rendition switching and relay-cache rewind. Design: [live-player.md](../../../../docs/architecture/browser-examples/live-player.md).

## Start a stream

External ffmpeg/OBS routes publish anonymously to `anon/live/test` — select **Public** in the viewer, enter `anon/live/test`, and press **Watch** (OBS/ffmpeg inputs require H.264 video + AAC audio). The built-in **MP4 Publish** publishes to whatever namespace the viewer displays (the temporary app's namespace by default unless **Public** is selected):

| Route                                         | Commands (separate terminals)                                                                                       | Relay URL in viewer                           |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- |
| Built-in MP4 publisher                        | Pick a file in **MP4 Publish** and click **Publish**                                                                | Default                                       |
| ffmpeg → cloud ingest → cloud relay           | `make ffmpeg-srt-bbb-remote`                                                                                        | `https://relay-1.moqt.research.skyway.io:443` |
| ffmpeg → local GStreamer ingest → cloud relay | `make gst-srt-publish GST_MOQT_URL=https://relay-1.moqt.research.skyway.io:443`, `make ffmpeg-srt-bbb-local`        | `https://relay-1.moqt.research.skyway.io:443` |
| ffmpeg → local ingest → local relay           | `make relay`, `make gst-srt-publish` (or `make ingest-gateway`), `make ffmpeg-srt-bbb-local` (or `make ffmpeg-srt`) | `https://127.0.0.1:4433`                      |

For OBS: SRT `srt://<host>:9000?mode=caller&streamid=anon/live/test` or RTMP `rtmp://<host>:1935/anon/live/test` (key `stream`), where `<host>` is `relay-1.moqt.research.skyway.io` or `localhost`. Override the viewer's default relay URL (`relay-1`) with `?moqtUrl=...`.

## Controls

Hover over the video to show controls:

| Control                                       | Action                                                                                                                                                                                                                              |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `↺5` `↺1` `1↻` `5↻`, ← / → (1 s), ↓ / ↑ (5 s) | Seek within the relay cache                                                                                                                                                                                                         |
| Seek bar                                      | Spans the broadcast; the thin bar below is the player's estimated replayable range (60 s for observed groups plus groups listed in the publisher's media timeline; `FETCH` can fall through to the publisher on a relay cache miss) |
| Center button                                 | Pause / resume                                                                                                                                                                                                                      |
| `LIVE`                                        | Jump back to live                                                                                                                                                                                                                   |
| Speed                                         | `0.5x`–`2x` during CMAF rewind                                                                                                                                                                                                      |
| Gear                                          | Video rendition, audio track, and packaging (`LOC` WebCodecs or `CMAF` MediaSource)                                                                                                                                                 |
| `Min buffer` / `Max buffer`, `Catch up`       | Adaptive jitter buffer bounds and catch-up mode (`Skip`, `Speed up`, `Off`)                                                                                                                                                         |

## MP4 Publish

Publishes a local MP4 from the browser using the same catalog and LOC tracks as `ingest-gateway` on a separate session:

- Video must be H.264; audio may be AAC or MP3 (other audio codecs are skipped).
- **Loop** replays from the start at EOF; the preview displays `pub to sub delay` below it.

## E2E

With relay, source (not needed for MP4), and Vite running:

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=live \
npm --prefix examples/browser run e2e:live-viewer     # or e2e:live-viewer-mp4
```

Delivery check (verifies zero missing samples over `DELIVERY_SECONDS`, default `30`, against the ingest log):

```shell
RUST_LOG=info,broadcast::delivery=debug make ingest-gateway 2>&1 | tee /tmp/ingest-gateway.log
DELIVERY_BRIDGE_LOG=/tmp/ingest-gateway.log \
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=anon/live/test \
npm --prefix examples/browser run e2e:live-viewer-delivery
```

# broadcast

Publishes and reads MSF broadcasts (`catalog` + LOC/CMAF tracks + timelines) over `moqt` using `mediapack::MediaEvent` streams. Shared by `ingest-gateway`, `moqtsink`, `egress-gateway`, Live Viewer's MP4 publisher, and the SDKs so one player works across all of them.

> **Building a standalone Rust application?** Use [`sdk/rust`](../../sdk/rust/README.md) (`moq-wasm`) for reconnects, token refresh, and a simpler broadcast API.

## Main entry points

| Type | Role |
| --- | --- |
| `RelayConnection`, `MoqtTarget` | Connects to a relay (or wraps an existing `moqt::Session`) and owns shared broadcast state |
| `Broadcast` | Namespace-scoped handle to announce, declare tracks, wait for subscribers, and send objects |
| `BroadcastWriter` | Push `mediapack::MediaEvent`s (`push()`) to publish LOC/CMAF tracks, media timelines, and catalog updates with backpressure handling |
| `MediaSource` | Pre-encoded frame/rendition publisher with optional `on_demand` publishing (used by the browser, Python, and Rust SDKs) |
| `BroadcastReader` | Subscribes to the LOC video and audio tracks selected by `TrackChoice`, yielding aligned `mediapack::MediaEvent`s |

## Track layout

| Track | Packaging | Content |
| --- | --- | --- |
| `catalog` | MSF (`draft-ietf-moq-msf-02`) | Track list; `initDataList` holds AAC config and CMAF init segments |
| `video`, `video_<height>p` | LOC (`draft-ietf-moq-loc-04`) | H.264 Annex-B with in-band SPS/PPS and capture timestamp |
| `audio` | LOC | Raw AAC, G.711 A-law, Opus, or MP3 frames |
| `<track>_cmaf` | CMAF (`draft-ietf-moq-cmsf-01`) | One `moof` + `mdat` fragment per sample (AAC only for audio) |
| `<video>_timeline` | MSF media timeline | Per-group presentation time, location, and encode wallclock |
| `chat`, event timelines | MSF event timeline | Publisher records indexed by wallclock or media time |

`video_720p` / `video_480p` / `video_360p` are added when transcoding ([`crates/transcode`](../transcode/README.md)). Customise track names via `SourceTracks` (e.g. `ingest-gateway pull` per camera profile).

## Behaviour

- Video keyframes open new groups; audio joins the current video group so group IDs align across tracks.
- Group IDs are wall-clock based so publisher restarts never reuse locations.
- LOC tracks are pushed with `PUBLISH` immediately (or only while subscribed when `on_demand` is enabled); CMAF tracks are served only while the relay subscribes to them.
- Media objects are retained for 60 s to answer relay `FETCH` misses; `catalog` keeps only the newest object (`Retention::NewestOnly`).
- `catalog` is re-sent on change and at least every 30 s.

Design: [architecture.md](../../docs/architecture/broadcast/architecture.md).

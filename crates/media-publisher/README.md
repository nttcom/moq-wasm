# media-publisher

Publishes an MSF catalog and H.264/AAC tracks into a MoQT relay. It is the
publishing side shared by `crates/moqt-bridge-live-ingest` and `crates/gst-plugin-moqt`
(`moqtsink`); the Live Viewer's in-browser MP4 publisher follows the same
conventions. Every publisher built on it produces the tracks below, so one
player, `examples/browser/examples/live-viewer`, plays and rewinds all of them.

## Tracks

| Track | Packaging | Content |
| --- | --- | --- |
| `catalog` | MSF (draft-ietf-moq-msf-01) | Track list. `initData` carries the AAC config and the CMAF init segments |
| `video`, `video_<height>p` | LOC (draft-ietf-moq-loc-01) | H.264 Annex-B, parameter sets in band, capture timestamp in extension header 2 |
| `audio` | LOC | Raw AAC frames |
| `<track>_cmaf` | CMAF (draft-ietf-moq-cmsf-01) | One `moof` + `mdat` fragment per sample |
| `timeline` | MSF media timeline | `[presentation time, [group, object], encode wallclock]` per keyframe |

`video_720p` / `video_480p` / `video_360p` appear when the publisher transcodes
(`crates/transcode`); each is one `altGroup` entry with `width` / `height`.

## Behaviour

- Groups start at video keyframes. An audio sample joins the group of the preceding keyframe, so one group id names the same instant on every track.
- Group ids are derived from the wall clock, so a restarted publisher never reuses a location.
- Every track except the CMAF siblings is sent with PUBLISH before the catalog that lists it, without waiting for a subscriber. CMAF tracks are sent only while the relay subscribes to them.
- Objects are kept for 60 s and answer standalone FETCHes the relay cannot cover from its own cache.
- The catalog is re-sent when it changes and every 30 s; its newest object is kept for the publisher's lifetime.
- A SUBSCRIBE for a track the catalog does not list is rejected with TRACK_DOES_NOT_EXIST.

The reasoning behind these choices is in the
[architecture document](../../docs/architecture/media-publisher/architecture.md).

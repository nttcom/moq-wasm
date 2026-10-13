# broadcast

Publishes an MSF broadcast, a catalog and H.264/AAC tracks, into a MoQT relay.
It is the publishing side shared by `crates/live-ingest`,
`crates/gst-plugin-moqt` (`moqtsink`), `crates/onvif-ingest` and, built into
`crates/wasm`, the Live Viewer's in-browser MP4 publisher. Every publisher built
on it produces the tracks below, so one player,
`examples/browser/examples/live-viewer`, plays and rewinds all of them.

## Tracks

| Track | Packaging | Content |
| --- | --- | --- |
| `catalog` | MSF (draft-ietf-moq-msf-01) | Track list. `initDataList` carries the AAC config and the CMAF init segments, each named by the `initRef` of its track |
| `video`, `video_<height>p` | LOC (draft-ietf-moq-loc-01) | H.264 Annex-B, parameter sets in band, capture timestamp in extension header 2 |
| `audio` | LOC | Raw AAC, G.711 A-law, Opus or MP3 frames (CMAF sibling for AAC only) |
| `<track>_cmaf` | CMAF (draft-ietf-moq-cmsf-01) | One `moof` + `mdat` fragment per sample |
| `timeline` | MSF media timeline | `[presentation time, [group, object], encode wallclock]` per keyframe |

`video_720p` / `video_480p` / `video_360p` appear when the publisher transcodes
(`crates/transcode`); each is one `altGroup` entry with `width` / `height`.
A publisher can name its tracks otherwise (`SourceTracks`): `onvif-ingest`
publishes `video/profile_N` and `audio/profile_N` per camera profile, without
`timeline`.

## Behaviour

- Groups start at video keyframes. An audio sample joins the group of the preceding keyframe, so one group id names the same instant on every track.
- Group ids are derived from the wall clock, so a restarted publisher never reuses a location.
- Every track except the CMAF siblings is sent with PUBLISH before the catalog that lists it, without waiting for a subscriber. CMAF tracks are sent only while the relay subscribes to them.
- Objects are kept for 60 s and answer standalone FETCHes the relay cannot cover from its own cache.
- The catalog is re-sent when it changes and every 30 s; its newest object is kept for the publisher's lifetime.
- A SUBSCRIBE for the catalog is accepted from the announce on, and one for a track declared with `Broadcast::declare_tracks` before the catalog lists it; `Broadcast::wait_for_subscriber` resolves on the first. Any other SUBSCRIBE for a track the catalog does not list as CMAF is rejected with TRACK_DOES_NOT_EXIST.

The reasoning behind these choices is in the
[architecture document](../../docs/architecture/broadcast/architecture.md).

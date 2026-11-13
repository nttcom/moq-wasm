# broadcast

Publishes an MSF broadcast, a catalog and its media tracks, into a MoQT
relay. It is the publishing side shared by `live-ingest`, `moqtsink`,
`onvif-ingest`, `moq-cli` and the Live Viewer's in-browser MP4 publisher, so
one player plays and rewinds all of them.

## Tracks

| Track | Packaging | Content |
| --- | --- | --- |
| `catalog` | MSF (draft-ietf-moq-msf-01) | Track list; `initDataList` carries the AAC config and the CMAF init segments |
| `video`, `video_<height>p` | LOC (draft-ietf-moq-loc-04) | H.264 Annex-B with in-band parameter sets and the capture timestamp |
| `audio` | LOC | Raw AAC, G.711 A-law, Opus or MP3 frames |
| `<track>_cmaf` | CMAF (draft-ietf-moq-cmsf-01) | One `moof` + `mdat` fragment per sample; AAC only for audio |
| `<video>_timeline` | MSF media timeline | Per group: presentation time, location and encode wallclock |

`video_720p` / `video_480p` / `video_360p` appear when the publisher
transcodes (`crates/transcode`). A publisher can name its tracks otherwise
(`SourceTracks`), as `onvif-ingest` does per camera profile.

## Behaviour

- Groups start at video keyframes, and audio joins the group of the preceding keyframe, so one group id names the same instant on every track.
- Group ids come from the wall clock, so a restarted publisher never reuses a location.
- Tracks are sent with PUBLISH without waiting for a subscriber; CMAF tracks only while the relay subscribes to them.
- Objects are kept for 60 s to answer FETCHes the relay cannot cover from its cache.
- The catalog is re-sent when it changes and every 30 s.

Design: [architecture document](../../docs/architecture/broadcast/architecture.md).

# `media-publisher` Architecture

## Status
Living document. Update this file in the same change whenever the track
layout, the publishing flow or the invariants described here change.

## Scope
`crates/media-publisher` turns `mediapack` media events into MoQT tracks: an
MSF catalog, LOC and CMAF media tracks, a media timeline and a publisher-side
FETCH cache. `crates/moqt-bridge-live-ingest` and `crates/gst-plugin-moqt` build on it. The
track layout is summarised in [its README](../../../crates/media-publisher/README.md);
this document records why the tracks are shaped that way.

## Publishing without subscribers
Every track the catalog lists except the CMAF tracks is published with PUBLISH
(draft-ietf-moq-transport-14 §9.13) before the catalog that lists it is sent, so
the relay ingests and caches each of them from its first object, and a viewer
subscribing to a track it read in the catalog is served by the relay. The CMAF
tracks are sent only while the relay holds a SUBSCRIBE for them, so the uplink
does not carry both packagings of every rendition while nobody watches CMAF;
the relay caches a CMAF track from its first subscriber on. PUBLISH_NAMESPACE
is still sent so that the relay can forward those SUBSCRIBEs here, and so that
relays in a cascade can route a SUBSCRIBE to the relay the publisher is
connected to. A SUBSCRIBE for any other track is rejected with
TRACK_DOES_NOT_EXIST.

## Track format
Video and audio objects are LOC (draft-ietf-moq-loc-01): the payload is the
codec bitstream, H.264 in Annex-B and raw AAC frames, and the capture timestamp
travels as MoQT extension header 2. The audio track's AudioSpecificConfig is
published Base64-encoded as the catalog `initData`; the video track carries its
parameter sets in band and has none.

## CMAF tracks
Every media track has a CMAF sibling named with a `_cmaf` suffix (`video_cmaf`,
`video_480p_cmaf`, `audio_cmaf`) declared with `packaging: cmaf`
(draft-ietf-moq-cmsf-01). Its init segment travels Base64-encoded in the catalog
`initData` (§3.1) and every object is one `moof` + `mdat` fragment holding one
sample (§3.3). Groups start on keyframes and take the same ids as the LOC track
for the same presentation time, so the LOC and CMAF versions of a rendition are
interchangeable; each format forms its own switching set (`altGroup` 1 for LOC,
2 for CMAF).

## FETCH cache
Every object is numbered and kept for 60 seconds from the moment it is
produced, whether or not anything is subscribed, and a standalone FETCH for a
cached range is answered from that cache (draft-ietf-moq-transport-14 §9.16).
The relay forwards a FETCH upstream when its own cache cannot cover the range,
so a viewer can rewind a CMAF track into the part that predates the relay's
first subscriber to it. A track published or subscribed while a group is open
starts sending at the next group so the live and cached object ids agree.

The catalog is the exception to the 60 seconds: its newest object is kept for
as long as the publisher runs. It is sent when it changes and again every 30
seconds, half the relay's default cache retention, so the relay always holds
one (draft-ietf-moq-msf-01 §5). A viewer that joins a subscription the relay
already holds does not wait for the next one on the SUBSCRIBE, which starts
after the largest object: it fetches the current catalog, and the relay
completes that FETCH from here when its own cache has dropped it.

## Group alignment
The source video track and its transcoded renditions form a CMSF switching set
(draft-ietf-moq-cmsf-01 §3.2): the transcoder is asked for a keyframe at every
source keyframe and each rendition group takes the group id the source assigned
to that presentation time, so the same group id names the same instant on every
track. A rendition that misses a source keyframe keeps writing into its current
group and announces the skipped ids with the Prior Group ID Gap header when it
catches up.

The audio tracks follow the same boundaries: an audio sample belongs to the
group of the latest video keyframe at or before it, so audio groups start at
the same keyframes with the same ids as the video groups. Audio before the
first keyframe is dropped as the video before it is. A viewer therefore replays
the audio of a video group range by fetching the same range on the audio
track, and a subscriber joining late starts both tracks at the same keyframe.

Group ids are seeded from the wall clock rather than counted from zero. The
relay keeps a track's cache when its publisher is replaced and treats a track
that reuses a known location as malformed, so a restarted publisher must never
produce a location an earlier run already used.

## Media timeline
The `timeline` track (draft-ietf-moq-msf-01 §7) carries, at every video
keyframe, the records `[presentation time, [group id, 0], encode wallclock]`
accumulated so far as one object in a new group. Records older than the relay's
cache retention are dropped. Viewers use it to place groups on a seek axis
that survives rendition changes, since renditions number their groups
independently.

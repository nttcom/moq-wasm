# `broadcast` Architecture

## Status
Living document. Update this file in the same change whenever the track
layout, the publishing flow or the invariants described here change.

## Scope
`crates/broadcast` turns `mediapack` media events into an MSF broadcast, the
unit draft-ietf-moq-msf-01 initiates and ends (§11.2, §11.3): a catalog, LOC and
CMAF media tracks, a media timeline and a publisher-side FETCH cache.
`crates/live-ingest`, `crates/gst-plugin-moqt`, `crates/onvif-ingest` and the
Live Viewer's MP4 publisher, through `crates/wasm`, build on it. The track
layout is summarised in [its README](../../../crates/broadcast/README.md); this
document records why the tracks are shaped that way.

Three types carry the three multiplicities. `RelayConnection` is one session
to the relay, opened on first use and shared by every broadcast on it: RTMP
publishes every stream key of one connection through the same session, and
the live-ingest stats panel reads its transport statistics and stream ledger.
`Broadcast` binds that connection to one namespace: it announces the
namespace, holds the catalog metadata and sends the objects of its tracks.
`BroadcastWriter` turns `mediapack::MediaEvent`s into those objects behind a
queue that never blocks the media source; `live-ingest` adds the transcoded
renditions through the same `Broadcast`. A writer writes into `video` and
`audio` by default; `onvif-ingest` runs one writer per camera profile in one
broadcast under `SourceTracks` naming `video/profile_N` and `audio/profile_N`.
The media timeline and the chat describe the default `video` and `audio`, so
the catalog lists them, and the `audio` placeholder sent before the AAC config
arrives, only while a default track exists.

Tasks are spawned and clocks read only through `moqt::executor`, so the crate
also builds for wasm32 and runs in a browser, where `RelayConnection` reaches
the relay through moqt's `BROWSER` transport (WebTransport) instead of `DUAL`.

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
connected to. A SUBSCRIBE for the catalog is accepted from the announce on,
before the source has produced one, and the catalog flows on it once it is
sent; `Broadcast::wait_for_subscriber` resolves on the first SUBSCRIBE accepted
in the namespace, so a source such as an ONVIF camera can be started only when
a viewer arrives. `Broadcast::declare_tracks` extends the same to tracks a
subscriber knows by name before any catalog lists them, such as the camera
video a tracking bot subscribes to directly. A SUBSCRIBE for any other track is rejected with
TRACK_DOES_NOT_EXIST.

## Track format
Video and audio objects are LOC (draft-ietf-moq-loc-01): the payload is the
codec bitstream, H.264 in Annex-B and raw AAC, G.711 A-law, Opus or MP3 frames,
and the capture timestamp travels as MoQT extension header 2. The capture
timestamp is the wall clock at which `BroadcastWriter::push` received the first
sample, advanced by each later sample's presentation time offset from that
sample, so the time the publish task spends connecting to the relay does not
move it into the future. A track's
initialization data is a Base64-encoded entry of the catalog `initDataList` that
the track's `initRef` names (draft-ietf-moq-msf-01 §5.1.7, §5.2.13); each entry
takes the name of its track as its id, which keeps the ids unique. An AAC audio
track's AudioSpecificConfig is published this way; A-law, Opus and MP3 need none
beyond the codec, sample rate and channel count the catalog lists, and the video
track carries its parameter sets in band.

## CMAF tracks
Every media track has a CMAF sibling named with a `_cmaf` suffix, except A-law,
Opus and MP3 audio, which travel as LOC alone (`video_cmaf`,
`video_480p_cmaf`, `audio_cmaf`) declared with `packaging: cmaf`
(draft-ietf-moq-cmsf-01). Its init segment is its `initDataList` entry (§3.1)
and every object is one `moof` + `mdat` fragment holding one sample (§3.3).
Groups start on keyframes and take the same ids as the LOC track for the same
presentation time, so the LOC and CMAF versions of a rendition are
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

The mechanism is `moqt::Origin` (see the `moqt` architecture document): it
numbers and caches the objects, writes them to the relay, and answers FETCH,
TRACK_STATUS, UNSUBSCRIBE and FETCH_CANCEL for the tracks registered with it.
This crate decides the policy: which tracks exist, the 60 second retention,
that the catalog keeps only its newest object, which tracks are published
eagerly and which wait for a SUBSCRIBE, and that a SUBSCRIBE for anything
else is rejected. The stream ledger behind the live-ingest stats panel is
updated from the write outcomes `Origin` reports.

The catalog is the exception to the 60 seconds: its newest object is kept for
as long as the publisher runs. It is sent when it changes and again every 30
seconds, half the relay's default cache retention, so the relay always holds
one (draft-ietf-moq-msf-01 §5). A viewer that joins a subscription the relay
already holds does not wait for the next one on the SUBSCRIBE, which starts
after the largest object: it fetches the current catalog, and the relay
completes that FETCH from here when its own cache has dropped it.

## Relay switchover
A relay that is shutting down sends GOAWAY with the URI of another relay, or
an empty URI meaning the same one (draft-ietf-moq-transport-14 §3.5, §9.4).
`RelayConnection` performs the publisher switchover of §8.4.2, publishing
on both relays until the old one lets go as draft-22 §7.6.1 does. The event loop
records the URI, and the next `connected()` call starts the move in a
background task while every send keeps using the current session: the task
connects a second session, announces every namespace, publishes the catalog
again (which re-sends PUBLISH for the LOC tracks and the catalog track) and
moves the subscriber watches, then takes over catalog changes made in the
meantime and makes the new session current.

After the move every object is written to both sessions, as
draft-ietf-moq-transport-22 §7.6.1 describes: viewers still on the old relay
keep receiving the track, and the new session starts each track at its next
group (an object within a group is dropped where no group is open). The
previous session is closed with NO_ERROR once its relay no longer subscribes
to any track (`Origin::is_serving`), which a draining relay of this repository
signals by unsubscribing when the subscribers it sent GOAWAY to have left, or
when that relay closes it. The new session gets a fresh `Origin`, so the FETCH
cache before the move is not carried over. If the named relay cannot be reached, the connection keeps publishing
to the old session until that relay closes it; the connect itself has no
timeout of its own.

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
cache retention are dropped. Viewers use it to place groups on a seek axis;
since every track of the switching set shares the source's group ids (see
Group alignment), the same records hold across rendition changes.

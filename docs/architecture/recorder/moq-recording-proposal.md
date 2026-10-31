# MoQ recording and playback proposal

This is a proposal, not the behavior of the current recorder. It records the
objects of a meeting so a player can reconstruct its media and event timelines
without compositing or re-encoding. MP4 export remains a separate operation.

## Boundary

The recorder subscribes through the relay, rather than treating the relay's
disk cache as an archive. That cache is optional, drops data under pressure,
and discards its index on restart. The existing room recording job already
discovers participant namespaces and follows their catalogs. Extend that job
to archive selected tracks, including catalogs and their changes. Capture
the room's transcript and each participant's chat track as well as selected
camera, screen share, and audio tracks. Record track availability and room
membership events when they are available; do not infer an exact join or leave
time from the first or last media frame.

The archive preserves the received objects, not the original transport
streams. Recording is limited to what the recorder subscribes to and actually
receives. Keep the existing MP4 recording path and its manifest unchanged;
use a separate archive manifest and version so existing composites continue
to work. This proposal does not specify changes to the relay protocol.

## Layout

Store an archive under the existing recording ID in the configured object
store. Use opaque track IDs in object keys; namespace and track names belong
in the manifest, not in paths whose escaping could collide.

```text
<recording-id>/archive/
  manifest.json
  tracks/<track-id>/00000000.data
  tracks/<track-id>/00000000.index.json
  tracks/<track-id>/00000001.data
  tracks/<track-id>/00000001.index.json
```

Each data chunk contains concatenated payload bytes, in receipt order. Its
index has one entry for every received object, including objects with an empty
payload. Each entry records the byte offset and length, group, subgroup and
object IDs as applicable, object status, priority, forwarding information,
extension headers in a lossless representation, and group-ending information.
It also records a receipt sequence number. These fields must cover the
canonical `moqt::object::Object` in the implementation, including fields
added to it later; unknown extension headers must survive a round trip.
Catalog and event payloads are stored unchanged. Chunk boundaries are chosen
by size or elapsed recording time, not assumed to be group or keyframe
boundaries. Lengths, offsets and IDs must be represented without loss in
JavaScript; JSON uses decimal strings for 64-bit integers.

The versioned manifest identifies the recording, its start and end, tracks
(namespace, name, role, participant when known), chunk keys, and completion
status. Each chunk entry includes a checksum and byte length for both data and
index. The index also supplies a time-to-object lookup for seekable media and
events, derived from the actual payload or media timeline; missing timestamps
remain explicitly unknown. The stored raw object metadata is authoritative;
the derived index may be rebuilt. Use unique immutable keys for closed chunks,
upload data and index first, then publish their manifest entry. Publish the
final manifest only after all accepted chunks have been accounted for. A
crash or upload failure must leave the recording visibly incomplete, never
silently claim it is complete. A partial recording can expose only chunks
whose data and index were successfully committed.

This is an application-specific archive, not an asserted standard MoQ file
format. Its exact serialization and resource limits need an implementation
review before the format is frozen.

## Time and seek

The player uses a recording-relative timeline derived from a declared common
clock. Preserve source capture timestamps and event timestamps as well as the
clock mapping and its uncertainty; never replace them with receipt time
without marking the fallback. Existing chat uses the sender's `Date.now()`,
and the transcriber publishes an utterance start time. These timestamps are
not proof that different machines' clocks are synchronized. The first version
must surface an unknown or excessive clock offset as approximate alignment,
not promise exact synchronization. A clock synchronization mechanism and an
acceptable error bound must be selected and measured before claiming accurate
cross-participant synchronization. Transcript ordering uses utterance time,
not the later time at which transcription arrives.

To seek to time T, load the applicable catalog and configuration at or before
T, find the preceding decodable video access point for each selected track,
and decode forward to T. Align audio to T on the same timeline. Replay event
records up to T, or load a state snapshot plus later events, so chat and other
stateful UI do not disappear on a backward seek. The index must distinguish
an absent interval, unavailable timestamp, and missing recorded data. If an
access point, configuration, or a required chunk is missing, show a gap or an
error instead of presenting an apparently complete recording. Do not assume
that every Group starts with a decodable keyframe.

## Playback

An authenticated HTTP request to the recorder starts replay for a recording.
The recorder reads the archive and publishes its tracks through the relay under
`<room>/recordings/<recording-id>/...`. Players connect to the relay and use
that namespace for playback. A recording ID separates simultaneous replays
from the live room and from other recordings. The recorder must define
historical catalog, range and timing behavior rather than assuming a live
subscription suffices. The existing browser player's LOC/CMAF decoding and
timeline logic are candidates for reuse, but its live and FETCH paths do not
already implement synchronized room playback.

Use one playback clock for the room. Decode and render selected participant
video tracks separately, mix their audio on that clock, and show chat and
transcript events as their times arrive. Selecting fewer video tracks can
reduce bandwidth and decoding work. Audio continues when a video tile is
hidden. Browser codec support and the cost of decoding multiple streams vary
by device; expose unsupported tracks and allow reducing visible video.

## Failure and access

Archive authorization covers the manifest, indexes, chunks and any export,
including byte-range requests; object keys alone are not authorization. Apply
the same retention and deletion policy to all archive objects and derived
exports. Do not store access tokens in the archive. Track a failed
subscription, dropped or unavailable object, upload failure, and clock
uncertainty as explicit incomplete ranges or recording errors. Detect duplicate
objects by track and object identity, preserve the first accepted payload, and
report conflicting duplicates rather than silently overwriting them. Chunk
checksums detect corruption, not missing upstream data.

## Delivery and validation

1. Record one participant's catalog and selected LOC audio/video objects,
   restart the reader, and play and seek through the relay. Verify object metadata and
   payload round-trip, catalog changes, keyframe seeks, and incomplete chunks.
2. Record multiple participants and screen shares, align them on one clock,
   and test joins, leaves, mute, gaps, clock offsets and device load.
3. Record chat and transcript tracks, preserve source timestamps and speaker
   identity, and verify seek reconstructs the event view, including late
   transcripts. Define event-specific edit/delete semantics before claiming
   they are supported.
4. Add optional MP4 export from the archive if needed. Measure recording CPU,
   storage, network and playback cost against the current per-participant MP4
   and composite paths with the same meeting; savings are not yet measured.

Before implementation, decide the common-clock mechanism and error target,
archive retention and authorization integration, and whether partial
recordings may be viewed. Validate the serialization against real object
variants and the targeted draft requirements before declaring interoperability.

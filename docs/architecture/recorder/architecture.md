# `recorder` Architecture

## Status
Living document. Update this file in the same change whenever the track
choice, the muxing flow or the file rules described here change.

## Scope
`crates/recorder` is a subscriber: it joins an MSF broadcast through a relay
and writes what it receives to local disk as fragmented MP4: one file holding
the video and the audio track, or with `--layout per-track` one file per
track. With `--upload` it moves every closed file to an object store. It knows the catalog (`msf`), LOC objects and
the fMP4 muxer (`mediapack`) and the client side of `moqt`; it owns no track
layout of its own and follows the one `crates/broadcast` publishes (see
[its README](../../../crates/broadcast/README.md)).

It is the first half of persisting what flows through a relay. The second
half, the relay spilling its object cache to disk and answering FETCH from
there, is a relay change and does not depend on this crate; once it exists,
`--from-cache` reaches back as far as the relay keeps.

## Why the LOC tracks and not the CMAF siblings
The CMAF siblings are already fMP4, and copying their fragments into a file
would need no media code. They were not chosen because they exist for AAC
audio only, each sibling is a single-track file so audio and video would end
up in two files, and a `broadcast` publisher sends them only while someone
subscribes, which is the cold path. The LOC tracks are always published and
carry every codec; `mediapack::loc::Demuxer` turns them into the same media
events the publishers started from, and `mediapack::mp4::Fmp4Muxer` writes
both tracks into one init segment and one fragment stream.

## Flow
1. `relay::RelayConnection` opens one session; the URL scheme selects QUIC or
   WebTransport (`Endpoint<DUAL>`), as `broadcast` does on the publishing side.
2. `catalog_reader::CatalogReader` subscribes to the catalog first and reads
   its current object with the joining FETCH (offset 0)
   draft-ietf-moq-msf-01 §5 asks for, falling back to the subscription when
   the publisher has sent none yet.
   `track_selection::select_tracks` picks the LOC track named by
   `--video-track` and `--audio-track`. Without a name it ranks the LOC
   tracks by their catalog role and quality fields instead of guessing names
   the publisher chose (a meeting participant lists `camera_main_720p` and
   `audio_128kbps`): among the video tracks whose codec can be recorded, the
   tallest at or below `--max-video-height` (480 by default; the shortest when
   all are taller), then the lowest `bitrate`, and the audio track with the
   highest `bitrate` among those that can be decoded; remaining ties go to
   the track listed first. The video default is a light rendition because a
   meeting participant encodes a rendition only while it has a subscriber
   and the recorder's SUBSCRIBE counts, so a rendition no viewer watches is
   encoded for the recording alone. Audio takes the highest bitrate, which
   viewers start on as well, and Opus is cheap to encode. The video codec
   comes from the catalog's `codec`, a WebCodecs codec string naming H.264, VP8, VP9 or AV1,
   the audio codec from its `codec`, `sampleRate` and `channelConfig`, and an AAC track's
   AudioSpecificConfig from the `initDataList` entry its `initRef` names, the
   way `broadcast::build_catalog` writes it. While the catalog lists no video
   track yet, the recorder waits for the next catalog object.
3. Both media tracks are subscribed with `NextGroupStart`, so the recording
   begins on a keyframe, and read with one `TrackReader` each. With
   `--from-cache` they are subscribed with `LargestObject` instead, and a
   standalone FETCH per track from `{0, 0}` through the Largest Location in
   SUBSCRIBE_OK is written first, in arrival order across the two tracks. The
   FETCH ends at that object and the subscription starts after it, so the
   two parts join without a gap or an overlap; the relay serves the range
   from its cache and FETCHes what it lacks from the publisher. The live
   objects wait in the readers' bounded channels, so QUIC flow control holds
   them back meanwhile.
4. `recording::Recording` holds one `pipeline::Pipeline` and one
   `file_writer::RecordingFile` per output file and routes each track's
   objects to the file that holds it. A pipeline converts each `TrackObject`
   to a `LocObject`, runs the track's `loc::Demuxer` and feeds the events to
   the `Fmp4Muxer`. It pushes one `Streams { has_video, has_audio }` for the
   tracks of its file and drops the single-track `Streams` events the
   demuxers announce, so the muxer waits for every configuration of the file
   before writing the init segment.
5. The `RecordingFile` appends the bytes and flushes whenever the file's
   leading track moved to a new group: the video, or the audio in a file
   without video. A video group is a complete GoP on disk.
6. The recording ends when either `TrackReader` returns `Ok(None)`. When the
   publisher leaves, the relay sends PUBLISH_DONE on both subscriptions and
   `moqt` ends them 10 s later, after any late streams, so the recorder needs
   no session-event handling of its own.

## Key invariants
- **One timeline for both tracks.** Each `loc::Demuxer` would otherwise count
  presentation time from its own first capture timestamp, and the two
  subscriptions start at different objects. The `Recording` keeps the
  capture timestamp of the first video object, and each pipeline creates its
  track's demuxer at its first object with `with_capture_origin` set to it.
  The files therefore keep the publisher's audio/video offset, whether the
  tracks share a file or not. Each subscription starts at its own next group,
  so the audio can start a group before the video. Audio captured before the
  origin is dropped, since the demuxer would clamp it to presentation time 0.
  Audio that overtakes the first video object waits for it, up to
  `MAX_PENDING_AUDIO` objects (about 10 s); beyond that the oldest is dropped.
- **An init segment never changes.** `Fmp4Muxer` rejects a configuration
  change after the init segment is out. The pipeline compares each
  `VideoConfig` / `AudioConfig` event with the one the current muxer was built
  from; a different one ends the file (the pending video sample is flushed
  with `finish`) and starts a fresh muxer, and the writer continues in the
  next numbered file.
- **Segments start on a keyframe.** With `--segment-seconds` the same rotation
  runs at the first keyframe whose presentation time is at least the segment
  length past the file's first video sample, so every file decodes on its
  own. A file without video rotates at the first audio frame past that
  length, since every audio frame is a sync sample. Presentation times are
  not rebased per file: the files keep one timeline and can be concatenated.
- **The file is valid at every flush.** Fragmented MP4 needs no trailing
  index, so a recorder killed mid-way leaves a playable file; only the last
  video sample, whose duration is unknown until the next one, is lost.
- **Never overwrite.** The output file is created with `create_new`; a name
  clash is an error, not a truncated recording.
- **Shutdown reaches the relay.** The session handle is dropped and
  `Endpoint::wait_idle` awaited before exit, so the CONNECTION_CLOSE is sent
  instead of the relay waiting for the idle timeout.

## Upload
With `--upload <URL>` the recorder builds an `ObjectStore` from the URL with
`object_store::parse_url_opts` and the process environment, so the credentials
never pass through flags (see [object_store.md](object_store.md)). Local disk
stays the write path, and the store only ever sees closed files:

- Each segment is a self-contained file, so it is uploaded the moment it
  closes, at rotation or at the end. `--upload` makes `--segment-seconds`
  default to 60 s; without segments the only file would close when the
  recording ends.
- `upload_task::UploadTask` uploads the closed files one at a time in closing
  order with `object_store::buffered::BufWriter`, which streams a large file
  as a multipart upload. It deletes the local file once the object is
  complete. A file whose upload still fails after the store's retries stays
  on disk with an error log, and the recording goes on.
- An object is named `<url path>/<namespace>/<track>/<recording start Unix
  ms>-<segment index>.mp4`, without `<track>` for the combined file. A `/` in
  the namespace or the track name nests the key.
- Every object carries the recording's capture origin as the metadata
  `capture-origin-us`: the Unix time in microseconds that presentation time
  0 of the file stands for. Files of different recordings can be placed on
  one wall clock from it.
- At the end the recorder waits for the queued uploads before it exits.

Writing straight into an in-progress multipart upload was not chosen: the
object is not readable until the upload completes, a crash loses the whole
segment, and a network stall would hold up the recording.

## Token refresh
With `--auth-token-file` a `TokenRefreshTask` re-reads the file every 10 s and
sends a changed token with the relay's in-session refresh convention: a
TRACK_STATUS for namespace `[app id]` and track `update_auth_token` carrying
the token as its AUTHORIZATION TOKEN (see the relay architecture document,
"Token refresh"). The app id is the namespace's first element. moq-cli does
the same; neither shares the code because draft-14 has no refresh message and
the convention is the relay's.

## Limits
- `--from-cache` reaches only as far back as the relay's and the
  publisher's caches, 60 s for a `broadcast` publisher. A relay that started
  caching mid-group returns that group without its keyframe, and the pipeline
  drops video until the first keyframe.
- Audio outside AAC, Opus, G.711 A-law and MP3, or an AAC track whose
  configuration is not in the catalog yet, is skipped with a warning.
- Video outside H.264, VP8, VP9 and AV1 ends the recorder with an error
  before anything is recorded: without the video track there is no file.
- The automatic choice reads no track name, so it cannot tell a meeting's
  camera from its screen share. At equal height the camera wins only
  because its bitrate is lower; a participant who shares the screen with
  the camera off is recorded from `screenshare_480p`.

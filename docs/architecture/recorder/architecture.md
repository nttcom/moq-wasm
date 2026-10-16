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
1. `relay::connect` opens one `moqt::MigratingSession`; the URL scheme selects
   QUIC or WebTransport (`Endpoint<DUAL>`), as `broadcast` does on the
   publishing side. When the relay drains, its GOAWAY moves every
   subscription to the relay it names and the files continue without a gap
   (see the `moqt` architecture, `MigratingSession`).
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
   way `broadcast::build_catalog` writes it. While the catalog lists neither
   a video track nor an audio track that can be decoded, the recorder waits
   for the next catalog object. While it lists only one of them, the
   recording starts with that track alone and keeps reading the catalog (see
   "Tracks that come and go"). With
   `--list-tracks` it prints the first catalog object's LOC tracks
   (`track_list::track_table`) to stdout and exits instead.
3. Both media tracks are subscribed with `NextGroupStart`, so the recording
   begins on a keyframe, and read with one `MigratingTrack` each. With
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
   leading track moved to a new group: the video while a video track is
   read, the audio otherwise. A video group is a complete GoP on disk.
6. The recording ends once the catalog track and every media track it reads
   have ended. When the publisher leaves, the relay sends PUBLISH_DONE on
   every subscription and `moqt` ends them 10 s later, after any late
   streams, so the recorder needs no session-event handling of its own. A
   media track that ends alone only stops that track (see "Tracks that come
   and go").

## Tracks that come and go
A publisher's first catalog often lacks the audio a later one lists:
`broadcast` sends a catalog at the first video and the first audio
configuration, and until the second it lists the AAC `audio` track without
its `initRef`; a meeting participant lists its audio tracks only while the
microphone is on. A room recording subscribes the moment the namespace is
published, so it reads exactly that first catalog. Waiting for the audio
before recording was not chosen, because a video-only `broadcast` lists the
`audio` placeholder for good and a muted participant may never unmute.

So the video starts alone, and while the recording has no audio,
`record_live` runs every catalog object through `track_selection::select_audio`.
The first track it returns is subscribed with `NextGroupStart` and added
with `Recording::add_audio`. In the combined layout the pipeline adds the
track to the muxer at once when nothing has been written yet; otherwise the
file goes on with video only, the audio meanwhile is dropped, and the next
keyframe starts the next numbered file with both tracks (the init segment
cannot change). The per-track layout opens the track's file. The catalog
branch is polled before the media branches, so a catalog that arrives
together with the first video object is applied before that object.

A meeting participant who mutes publishes a catalog without its audio
tracks and then ends them with PUBLISH_DONE; one who unmutes publishes a
catalog that lists them again and serves new SUBSCRIBEs. Ending the
recording with the audio was not kept: the video after a mute would be
lost. So `record_live` drops the audio subscription once a catalog no
longer lists its track, or once the track ends, and the video goes on. The
recording then reads no audio, and the next catalog that lists an audio
track adds it the same way. Dropping on the catalog matters because `moqt`
reports the end of a track 10 s after its PUBLISH_DONE: an unmute within
those 10 s would list the audio while the ended subscription is still
held, and no later catalog would follow.

An audio track added after an ended one goes to the file the ended one
went to. A file keeps its init segment, so audio with the configuration it
holds continues in it after a gap: every audio sample is its own fragment
whose decode time comes from its capture timestamp, so the gap needs
nothing written and the audio stays in sync. Audio with another
configuration starts the next file, at the next keyframe in a file with
video, at once in the per-track audio file. The per-track audio file keeps
the name of the first audio track.

A participant who turns the camera off publishes a catalog without its
camera tracks and ends them with PUBLISH_DONE; turning it on lists them
again, and the namespace is not published again. Ending the recording with
the video was not kept: a room recording starts a participant only on
PUBLISH_NAMESPACE, so the participant would never be recorded again. The
video therefore follows the catalog like the audio: `record_live` drops the
subscription once a catalog no longer lists the track, or once it ends, and
while the recording reads no video, each catalog object goes through
`track_selection::select_video`, the same choice as at start. The track it
returns is subscribed with `NextGroupStart` and added with
`Recording::add_video`.

While no video is read, the audio is recorded alone and leads the file: it
flushes at each audio group and ends a segment at the first frame past its
length. Holding the audio back until the video returns was not chosen, since
what a participant says with the camera off would be lost. For the same
reason a participant who joins with the camera off is recorded from the
audio: `select_tracks` returns a selection while the catalog lists either
track, and the capture origin is then the first audio object's capture
timestamp.

A video track added after an ended one goes to the file the ended one went
to, through a new demuxer. Its first keyframe continues the file when the
init segment holds the same configuration; the muxer gives the last sample
before the gap the gap's duration, so a player shows that frame until the
camera is back. Another configuration starts the next file at that
keyframe, and so does the first keyframe in a combined file that holds no
video because the recording started with the audio; the audio until then
stays in the audio-only file. The per-track video file keeps the name of
the first video track.

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
  While no video track is read, the first audio object sets the origin,
  including one held for a video track that ended before its first object.
- **An init segment never changes.** `Fmp4Muxer` rejects a configuration
  change after the init segment is out. The pipeline compares each
  `VideoConfig` / `AudioConfig` event with the one the current muxer was built
  from; a different one ends the file (the pending video sample is flushed
  with `finish`) and starts a fresh muxer, and the writer continues in the
  next numbered file.
- **Segments start on a keyframe.** With `--segment-seconds` the same rotation
  runs at the first keyframe whose presentation time is at least the segment
  length past the file's first video sample, so every file decodes on its
  own. While no video track is read, a file rotates at the first audio frame
  past that length, since every audio frame is a sync sample. Presentation times are
  not rebased per file: the files keep one timeline and can be concatenated.
- **The file is valid at every flush.** Fragmented MP4 needs no trailing
  index, so a recorder killed mid-way leaves a playable file; only the last
  video sample, whose duration is unknown until the next one, is lost.
- **Never overwrite.** The output file is created with `create_new`; a name
  clash is an error, not a truncated recording.
- **Shutdown reaches the relay.** `MigratingSession::close` closes the
  session and awaits `Endpoint::wait_idle` before exit, so the CONNECTION_CLOSE is sent
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

## Recording API
`recorder serve` keeps one HTTP API (`api_server::ApiServer`, see
[hyper.md](hyper.md)) and a `recordings::Recordings` list. Each
`POST /recordings` with `{"namespace_prefix", "layout"}` starts a
`room_recording::RoomRecording`:

- It opens its own relay session and sends SUBSCRIBE_NAMESPACE for the
  prefix. Each PUBLISH_NAMESPACE under it, the participants already there and
  those who join later, starts `record_broadcast` for that namespace with the
  requested layout. The tracks are picked from the catalog as without
  `--video-track` (`--max-video-height`, 480 by default), so a participant's
  browser encodes at most one extra light rendition for the recorder.
- A forwarded PUBLISH is refused with PUBLISH_ERROR UNINTERESTED. Accepting
  it would deliver every track of the namespace, every rendition included.
- A participant who leaves ends their tracks with PUBLISH_DONE, which ends
  their recording. One who joins again is recorded into new files. Muting
  and turning the camera off and on keep the recording going (see "Tracks
  that come and go").
- When the relay drains, the session sends SUBSCRIBE_NAMESPACE again on the
  relay its GOAWAY names; the PUBLISH_NAMESPACE that relay answers with for a
  participant already recorded is acknowledged and changes nothing.
- `POST /recordings/{id}/stop` sends the stop signal every
  `record_broadcast` watches. The state goes from `stopping` to `finished`
  once each file is closed and, with `--upload`, uploaded. The objects go
  under `<url path>/<recording id>/<namespace>/`.
- A recording also stops by itself, the same way, `--max-recording-seconds`
  (2 h) after it started, or once no participant has been recorded for
  `--empty-room-seconds` (5 min), counted from the start too. A participant
  counts until their recording ends, which is up to 10 s after the
  PUBLISH_DONE that follows their leaving, so a forgotten stop or an empty
  room cannot keep a recording running.
- SIGINT or SIGTERM stops every recording the same way before the process
  exits.

The API has no authentication, so it accepts only a prefix of at least two
elements under `--namespace-root` (`anon` by default), at most
`--max-recordings` recordings at once and one recording per prefix. A POST
whose `Origin` is not an `--allowed-origin` is refused, since CORS alone only
hides the answer from that page and a `text/plain` POST needs no preflight. The relay URL, the bucket and the
track choice are server settings, so a request cannot point the recorder at
another relay or bucket.

## Token refresh
With `--auth-token-file` a `TokenRefreshTask` re-reads the file every 10 s and
sends a changed token with the relay's in-session refresh convention: a
TRACK_STATUS for namespace `[app id]` and track `update_auth_token` carrying
the token as its AUTHORIZATION TOKEN (see the relay architecture document,
"Token refresh"). The app id is the namespace's first element. moq-cli does
the same; neither shares the code because draft-14 has no refresh message and
the convention is the relay's. The token goes to the current session, so it
follows a migration; the session to the new relay is opened with the token
the recorder started with.

## Limits
- `--from-cache` reaches only as far back as the relay's and the
  publisher's caches, 60 s for a `broadcast` publisher. A relay that started
  caching mid-group returns that group without its keyframe, and the pipeline
  drops video until the first keyframe.
- Audio outside AAC, Opus, G.711 A-law and MP3 is never recorded.
- An audio track listed after the first video was written starts at the
  next keyframe in the next file; the audio before it is lost. Only a missing
  track is added later: a track once picked stays while the catalog lists it.
- A recording that starts with the audio alone writes it to a file of its
  own in the combined layout; the first keyframe starts the next. A
  `broadcast` publisher whose first audio configuration comes before its
  first keyframe lists the audio first, so a recording that starts with that
  publisher can begin with a short audio-only file.
- Video outside H.264, VP8, VP9 and AV1 in the catalog a recording starts
  from ends the recorder with an error before anything is recorded. Listed
  later, it is logged and the recording goes on without it.
- The automatic choice reads no track name, so it cannot tell a meeting's
  camera from its screen share. At equal height the camera wins only
  because its bitrate is lower; a participant who shares the screen with
  the camera off is recorded from `screenshare_480p`, and stays so after
  the camera is turned on while the screen share is listed.

# `recorder` Architecture

## Status
Living document. Update this file in the same change whenever the track
choice, the muxing flow or the file rules described here change.

## Scope
`crates/recorder` is a subscriber: it joins an MSF broadcast through a relay
and writes what it receives to local disk as one fragmented MP4 file holding
the video and the audio track. It knows the catalog (`msf`), LOC objects and
the fMP4 muxer (`mediapack`) and the client side of `moqt`; it owns no track
layout of its own and follows the one `crates/broadcast` publishes (see
[its README](../../../crates/broadcast/README.md)).

It is the first half of persisting what flows through a relay. The second
half, the relay spilling its object cache to disk and answering FETCH from
there, is a relay change and does not depend on this crate; once it exists a
recorder that joined late can also FETCH the part it missed.

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
2. The catalog is subscribed first and its current object read with the
   joining FETCH (offset 0) draft-ietf-moq-msf-01 §5 asks for, falling back to
   the subscription when the publisher has sent none yet.
   `track_selection::select_tracks` picks the LOC track named by
   `--video-track` and `--audio-track`; the audio codec comes from the
   catalog's `codec`, `sampleRate` and `channelConfig`, and an AAC track's
   AudioSpecificConfig from the `initDataList` entry its `initRef` names, the
   way `broadcast::build_catalog` writes it. While the catalog lists no video
   track yet, the recorder waits for the next catalog object.
3. Both media tracks are subscribed with `NextGroupStart`, so the recording
   begins on a keyframe, and read with one `TrackReader` each.
4. `pipeline::Pipeline` converts each `TrackObject` to a `LocObject`, runs the
   track's `loc::Demuxer` and feeds the events to the `Fmp4Muxer`. It pushes
   one `Streams { has_video, has_audio }` itself and drops the single-track
   `Streams` events the demuxers announce, so the muxer waits for both
   configurations before writing the init segment.
5. `file_writer::RecordingFile` appends the bytes and flushes whenever the
   video moved to a new group, which is a complete GoP on disk.
6. The recording ends when either `TrackReader` returns `Ok(None)`. When the
   publisher leaves, the relay sends PUBLISH_DONE on both subscriptions and
   `moqt` ends them 10 s later, after any late streams, so the recorder needs
   no session-event handling of its own.

## Key invariants
- **One timeline for both tracks.** Each `loc::Demuxer` would otherwise count
  presentation time from its own first capture timestamp, and the two
  subscriptions start at different objects. The pipeline takes the first
  capture timestamp either track delivers and creates each track's demuxer
  at its first object with `with_capture_origin` set to it, so the file keeps
  the publisher's audio/video offset.
- **An init segment never changes.** `Fmp4Muxer` rejects a configuration
  change after the init segment is out. The pipeline compares each
  `VideoConfig` / `AudioConfig` event with the one the current muxer was built
  from; a different one ends the file (the pending video sample is flushed
  with `finish`) and starts a fresh muxer, and the writer continues in the
  next numbered file.
- **The file is valid at every flush.** Fragmented MP4 needs no trailing
  index, so a recorder killed mid-way leaves a playable file; only the last
  video sample, whose duration is unknown until the next one, is lost.
- **Never overwrite.** The output file is created with `create_new`; a name
  clash is an error, not a truncated recording.
- **Shutdown reaches the relay.** The session handle is dropped and
  `Endpoint::wait_idle` awaited before exit, so the CONNECTION_CLOSE is sent
  instead of the relay waiting for the idle timeout.

## Limits
- The authorization token is sent once and not refreshed.
- Recording starts live; there is no FETCH of the past yet.
- Audio outside AAC, Opus and G.711 A-law, or an AAC track whose
  configuration is not in the catalog yet, is skipped with a warning.

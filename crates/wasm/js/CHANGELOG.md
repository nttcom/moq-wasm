# Changelog

## Unreleased

- `LivePlayer` plays a broadcast none of whose catalog tracks is live
  (`isLive: false`), such as a recorder replay, as VOD: it subscribes to no
  media track, reads the media timeline with a joining FETCH, plays from the
  timeline's first group through FETCH, and pauses at the end.
  `LivePlayerState.isLive` and `LivePlayerState.ended` say so; the seek axis
  ends `trackDuration` after the timeline's first record, `goLive` does
  nothing, `PlayerControls` hides `LIVE`, and resuming after the end plays
  from the start. `MediaCatalogTrack` carries `isLive` and `trackDuration`.
- `init({ telemetry: { url, page } })` sends the wasm module's logs (at
  `logLevel` or above) and spans to the observability server at `url` every
  10 s and when the page is hidden, each session's under its token. The page's
  own logs are not sent.
- `onLog` (and `telemetry`) also receives the logs of the SDK's TypeScript,
  including its encoder and decoder workers and what `LivePlayer` and
  `PlayerControls` hand their `onLog`, with a `moq-wasm::…` `target` and no
  token. The console messages they replace now read
  `[<target>] <message> <field>=<value>…`.
- `LivePlayer` subscribes to audio with subscriber priority 0 and to video
  with 128, so a relay short of downlink bandwidth sends the audio first.
- `LivePlayerCallbacks.onAudioLevel` receives the LOC Audio Level of each
  live audio object that carries one, and `moq-wasm/media` exports its type
  `AudioLevel`, so a page can tell who is speaking.
- `LivePlayer` under automatic quality plays no rendition taller than its
  `container` needs: it watches the container's height in device pixels and
  starts on, steps down to or raises to the first rendition at least that tall.
- `init({ onLog, onSpan })` calls both with the token of the record's
  `MoqSession` as a second argument (the provider's newest), and `undefined`
  for a record of no session or of a session without a token, so a page can
  send each session's telemetry under its own token.
- `LivePlayer` starts its video subscription with a joining FETCH, from the
  first object of the group the largest object is in, so a picture shows as
  soon as playback starts even when its publisher sends no new group, such as
  a still screen share. A switch between renditions still waits for the next
  group.

## 0.2.0

- `LiveStats.arrivalDelayMs` gives the median delay from capture to arrival
  of the live audio and video over the last 10 s; their difference tells
  which kind lags the other and by how much.
- `publish()` stamps each encoder's chunks from the capture time of its first
  frame instead of from when the encoder started; until now a camera whose
  first frame came seconds after the encoder started had its video stamped
  that much early, and receivers played audio that much late.
- The audio `publish()` sends keeps its capture timestamps after a gap in the
  microphone's timestamps; until now each one was off by the gap from then on.
- `moq-wasm`: `init({ wasmUrl, onLog, logLevel })` loads the wasm module and
  hands its logs to `onLog` as `LogRecord`s at `logLevel` (default `'warn'`)
  or above.
- `moq-wasm/media` exports the types `LivePlayer` takes and returns
  (`BufferPolicy`, `CatchUp`, `LivePictureKind`, `LiveStats`,
  `DeliveryObserver`, `Playhead`, `StreamKind`), `formatLiveStats`, and
  `monotonicUnixMicros`, the clock of the capture timestamps.
- `publish()` accepts a SUBSCRIBE for the `video` and `audio` of the stream it
  is given before their first frame; `MediaBroadcast.declareTracks` does the
  same for any track.
- `LivePlayerOptions.source` plays one source of a broadcast, named by its
  video and audio tracks (`LiveSource`), such as a shared screen next to a
  camera; its audio keeps playing while its video is absent.
- `LiveStats.videoDroppedFrames` and `LiveStats.videoLateFrames` count,
  since playback started, the video frames never shown and the frames shown
  more than a frame period late; they replace `videoDrops`, which gave both
  in one string.

## 0.1.0

- `moq-wasm`: `MoqSession` connects to a relay over WebTransport, falling back
  to WebSocket when WebTransport does not connect, and sends SUBSCRIBE, FETCH, TRACK_STATUS,
  SUBSCRIBE_NAMESPACE and PUBLISH_NAMESPACE (draft-ietf-moq-transport-14).
  `publishTrack` returns a `TrackWriter` that numbers, caches and sends objects
  and answers the track's requests.
- `moq-wasm/media`: `publish()` sends a `MediaStream` as an MSF broadcast with
  LOC tracks; `LivePlayer` and `PlayerControls` play one back. Chrome, Edge and
  Safari. This entry point may change in any minor version.

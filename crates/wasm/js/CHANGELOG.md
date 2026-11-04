# Changelog

## Unreleased

- `init({ onLog, onSpan })` calls both with the token of the record's
  `MoqSession` as a second argument (the provider's newest), and `undefined`
  for a record of no session or of a session without a token, so a page can
  send each session's telemetry under its own token.

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

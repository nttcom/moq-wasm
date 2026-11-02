# Changelog

## Unreleased

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

## 0.1.0

- `moq-wasm`: `MoqSession` connects to a relay over WebTransport, falling back
  to WebSocket when WebTransport does not connect, and sends SUBSCRIBE, FETCH, TRACK_STATUS,
  SUBSCRIBE_NAMESPACE and PUBLISH_NAMESPACE (draft-ietf-moq-transport-14).
  `publishTrack` returns a `TrackWriter` that numbers, caches and sends objects
  and answers the track's requests.
- `moq-wasm/media`: `publish()` sends a `MediaStream` as an MSF broadcast with
  LOC tracks; `LivePlayer` and `PlayerControls` play one back. Chrome, Edge and
  Safari. This entry point may change in any minor version.

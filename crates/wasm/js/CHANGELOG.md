# Changelog

## Unreleased

- `createBroadcast()` replaces `publish()`, which is removed with `Publication`,
  `PublishOptions` and `SourcePublication`. It announces a namespace, and `addSource(name, input)` publishes each source, a camera
  included, in the tracks `name`, `name_audio` and `name_<rendition>`. A source
  takes devices the SDK captures, the page's tracks, a stream or the screen;
  `LocalSource` switches or releases a device, mutes it, shows the published
  tracks as `stream` and reports encoder `stats()`.
- `watchBroadcasts()` plays every broadcast under a prefix but the session's
  own, as a `RemoteBroadcast`: one catalog subscription and one view per source
  the catalog lists, with an element the SDK owns, its `state`, volume and
  stats. A view subscribes its video only while its element is on screen.
- `RemoteBroadcastOptions.audioContext`, which `watchBroadcasts` passes on,
  plays every view's live sound through the page's `AudioContext`.
- A player limited to a source plays the alternates of its audio too, such as
  translated audio.
- `MoqSession.transportStats()` returns the bytes the session has sent and
  received over all its connections (`sentBytes`, `receivedBytes`), so a page
  can show its send and receive rate next to `encodedKbps`. Over WebTransport
  the SDK counts them itself; they are the bytes the browser accepted, with
  no round-trip time or loss.
- `stats()` on each source returns
  the video and audio that source's encoders output over the last second
  (`encodedKbps`, `encodedFramesPerSecond`), the encoded video `frameSize`
  and that of each rendition, and, since it was published, the frames
  dropped before encoding and the encoder failures.
- `LivePlayer` plays a broadcast with audio and no video, such as a member
  who joined with the camera off: `LiveTracks.video` is optional, the playback
  status names the audio track, and the PUBLISH_DONE of the audio takes the
  tracks given to `start` back to waiting for the publisher, as that of the
  video does. A player whose video `setVideoEnabled(false)` turned off
  behaves alike. `start` rejects given tracks that list neither. Rewind and
  VOD still need a video track.
- `LivePlayerOptions.audioContext` plays the live LOC sound through the page's
  `AudioContext` instead of one per player, so players on a page share one
  output stream and the page picks the speaker with `setSinkId` on it. The
  player never suspends or closes it. Review and CMAF sound do not play
  through it.
- `LivePlayer.dispose()` stops the player and releases its decoder workers,
  audio contexts and pictures, so a page that creates a player per broadcast
  it shows does not keep them after the broadcast leaves.
- `LivePlayer.setVideoEnabled(false)` unsubscribes the live video and leaves
  the audio playing, so a page that hides the picture lets the publisher stop
  sending it; `setVideoEnabled(true)` subscribes it again from its current
  group.
- `BroadcastCatalog` subscribes the catalog of one broadcast once for every
  reader on the session, and `LivePlayer.start(session, namespace, catalog)`
  listens to it instead of subscribing the catalog itself. Players that play
  sources of one broadcast (`LivePlayerOptions.source`) share one, as
  draft-ietf-moq-transport-14 §9.7 forbids a second subscription to the same
  track on one session; the page can listen to it as well.
- `publish()` passes every field of `videoEncoderConfig` to the encoder and
  fills the unset ones with the package's defaults, so `latencyMode: 'quality'`
  now reaches the encoder instead of always `'realtime'`. Renditions take the
  same fields, with their own size and bitrate.
- `videoEncoderConfig` and `setVideoEncoderConfig` take a
  `Partial<VideoEncoderConfig>`, so a page may set only the bitrate and frame
  rate. `width` and `height` (1280x720 by default) bound the encoded size: a
  frame smaller than them is encoded at its own size instead of scaled up.
- `init()` can be called more than once, as React's StrictMode does: a later
  call returns the first call's promise and ignores its own options, instead
  of rejecting with "the log sink is already set". A call after the first
  one rejected starts again.
- `init({ telemetry: { redact } })` rewrites the fields of each log and span
  before they are sent, so a page can mask or drop a `track_namespace` that
  holds a user id.
- `MoqSubscription.close()`, `MoqNamespaceWatch.close()` and
  `Announcement.close()` resolve instead of rejecting with "connection lost"
  when the session has already ended.
- `LivePlayer`'s live playout buffer has no minimum by default: it is as long
  as the arrival spread plus the audio output latency, instead of at least
  200 ms. A page that wants a floor sets `minimumMs` with `setBufferPolicy`.

## 0.3.0

- `MoqSession.connect(url, { versions })` offers the MoQT drafts `versions`
  names (`MoqVersion`: `'draft-14'`, `'draft-18'`), most preferred first;
  without it only draft-14 is offered, as before.
- `MediaCatalogTrack` carries `eventType`, which names the records of an event
  timeline track (draft-ietf-moq-msf-01 §5.2.5), so a page can tell a chat
  from a transcript.
- `MediaCatalogTrack` carries `namespace`, set when a track is published
  under another namespace than its catalog's (draft-ietf-moq-msf-01 §5.2.2),
  and `LivePlayer` subscribes to the track there. It also carries `lang`.
- The catalog `publish()` sends gives its `version` as the string
  `"draft-01"` (draft-ietf-moq-msf-01 §5.1.1) instead of the number `1`.
- `init({ onSpan })` receives the wasm module's closed spans as `SpanRecord`s,
  and a `LogRecord` logged inside one carries its `traceId` and `spanId`.
- `RequestError.reasonPhrase` holds the reason phrase of the rejection.
- `publish()` reports through `onError` an encoder worker that fails to load
  or throws, and reports a failure whose `String()` equals the previous one
  only once.
- `moq-wasm/recording`: `openRecording(session, recorderUrl, id, { token })`
  asks the recorder to replay a finished recording and holds the answer of
  `POST /recordings/{id}/publish`, which keeps the replay running, until
  `close()` or until the session closes. `Recording.prefix` is the namespace
  prefix of the replay's broadcasts, and `Recording.startedAtMs` the Unix time
  in milliseconds the recording started, from which the replay's LOC
  Timestamps count as media time. Every `renew_after_seconds` it opens a new
  answer and closes the previous one a minute later, and it replaces an answer
  that ends on its own; `Recording.closed` gives the error when it could not.
  A refused request rejects with a `RecorderError` carrying the HTTP `status`.
  `token` is a string or an async function called before each request.
- `PlaybackGroup` plays several `LivePlayer`s at one position, a LOC
  Timestamp (the capture time live, media time in a recording), on one clock
  and one AudioContext: `add`, `remove`, `play`,
  `pause`, `seek`, `skip`, `goLive` and `state` (`mode`, `positionMicros`,
  `paused`, `ended`). While live every player plays at its own live edge;
  rewinding and VOD align them. A player whose range does not cover the
  position fetches nothing and reports `LivePlayerState.inRange` false. A
  player's own `seek`, `skip`, `goLive` and `setPaused` act on its group while
  it is in one.
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

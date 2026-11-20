# Changelog

## 0.8.0

- Breaking: `LiveStats.receivedKbps` is now `{ video: number; audio: number }`, and `receivedAudioKbps` is removed:

  ```ts
  // before
  stats.receivedKbps
  stats.receivedAudioKbps
  // after
  stats.receivedKbps.video
  stats.receivedKbps.audio
  ```

## 0.7.0

- Breaking: catalogs are now draft-ietf-moq-msf-02. `parseCatalog` rejects a catalog whose `version` is not `draft-02`, or whose namespace, `depends` or required fields (§5.2.4.1) are invalid. Publishers and players must be updated together.

- Breaking: `MoqSession.subscribeGroupStart` takes `priority` and `deliveryTimeoutMs` in an options object instead of a positional `priority`:

  ```ts
  // before
  await session.subscribeGroupStart(ns, track, 60, 7)
  // after
  await session.subscribeGroupStart(ns, track, 60, { priority: 7 })
  ```

- Breaking: `Mp4Broadcast` is created on a session with
  `MoqSession.mp4Broadcast(namespace, index)` instead of
  `new Mp4Broadcast(url, namespace, index)`, so it publishes with the
  session's token and versions. It no longer has `close()`; close the session
  instead.

  ```ts
  // before
  const broadcast = new Mp4Broadcast(url, namespace, index)
  await broadcast.close()
  // after
  const session = await MoqSession.connect(url, { token })
  const broadcast = session.mp4Broadcast(namespace, index)
  session.close()
  ```

- The stats of every started `LivePlayer`, including the players of a
  `RemoteBroadcast`'s views, are sent with `init({ telemetry })` without any
  code in the page: each under the token of the player's session, so they
  land in that session's app instead of `anon`, with the player's namespace
  as `trackNamespace` and its source's video track name as `mediaSource`.
  Set `telemetry.playerStats: false` to stop them. A page that sends its
  players' stats through `telemetry.stats` should drop them there, or they
  are sent twice.

- Fixed a `LivePlayer` track keeping the old Forward State when playback paused or resumed forwarding while its SUBSCRIBE was in flight: it stayed silent after a resume, or kept delivering during a review.

- `moq-wasm/media` exports `SeekAxis`, `StatusState`, `CatalogTrackRole`, `LogLevel` and `ReviewFetchWindow`, the types `LivePlayer`, `PlayerStatus` and `extractCatalogTracks` refer to.

- Fixed a video rendition of `publish()` logging a warning for every frame after the session ended; it now stops at the first failure and reports it as the main video does.

- Fixed a source sometimes sending no video or audio to a subscriber when the relay asked for a track right after accepting its PUBLISH.

- `depends` of a parsed catalog track lists track names; a `renderGroup` number is resolved to the names of its tracks.

- The MSF catalog of a `MediaBroadcast` now lists a `bitrate` for every audio and video track and a `framerate` for every video track, estimated when the encoder does not set them (30 fps when the frame rate is not capped).

- A player subscribes its catalog at Subscriber Priority 0, its text tracks (the media timeline, captions, `addTextTrack`) at 16 and its audio (`addAudioTrack` included) at 32; the catalog, the text tracks and an `addAudioTrack` track were at 128, the selected audio at 0. The media timeline also takes the player's `deliveryTimeoutMs` and `groupOrder`. On a congested downlink the unsent streams of the catalog and the media timeline no longer keep a prioritized video from being sent. `setVideoPriority` of 32 or less now puts a video level with or ahead of the audio, 16 the text tracks, 0 the catalog.

- `MoqSession.subscribeCurrentGroup` takes the options of `subscribeGroupStart` (`priority`, `deliveryTimeoutMs`, `groupOrder`).

- On draft-18 sessions, `MoqSubscription.setForward`, `setPriority` and `endBeforeGroup` now resolve once the relay has applied the update, so objects published right after `setForward(true)` are delivered. They reject if the relay refuses the update, does not answer within 10 s, or the subscription ends first.

- `addSource` takes `cmaf: true` to also send the video and its renditions as
  CMAF in `<track>_cmaf`, which a `LivePlayer` plays after
  `setPackaging('cmaf')`. The audio stays LOC only, so CMAF playback is video
  only.

- `MoqSubscribeOptions`, `LivePlayerOptions` and `RemoteBroadcastOptions` take
  `groupOrder` (`'ascending'` by default): the Group Order a SUBSCRIBE
  carries. With `'descending'` a relay sends the newest group first when a
  congested link holds more than one, so live playback jumps ahead instead
  of falling behind.

- `LivePlayer.setVideoPriority` called while the first video SUBSCRIBE is still unanswered is no longer dropped: the track kept its previous Subscriber Priority until the next change.

- Added `deliveryTimeoutMs` to `MoqSubscribeOptions`, `LivePlayerOptions` and `RemoteBroadcastOptions`: the DELIVERY TIMEOUT a SUBSCRIBE carries. A relay that honours it gives up on an object that many milliseconds after receiving it and resets the stream of its group, so a congested downlink carries recent media instead of a growing backlog. A player applies it to its selected video and audio track, not to tracks added with `addAudioTrack`, and throws a `RangeError` unless it is an integer from 1 to 4294967295. Without it every object is delivered, as before.

- `buildMsfCatalogJson(tracks, initDataList?)` and its `MsfTrack` / `MsfInitData` types are exported from `moq-wasm/media`, for a page that publishes its own media tracks with `publishTrack` and needs an MSF catalog `LivePlayer` can read.

- A draft-14 server whose SERVER_SETUP carries a parameter that is not UTF-8 no longer crashes the page's wasm module.

- Fixed: after a GOAWAY, the SDK no longer subscribes on the new relay to a track whose publisher ended it with PUBLISH_DONE.

- Fixed: when the relay a GOAWAY names rejects a published track, the track now ends with an unsubscribe once the previous relay closes, instead of staying with no subscriber and failing every write.

- Fixed: a subscription moved by GOAWAY from a draft-14 relay to a draft-18 relay no longer fails as a malformed track when it receives a datagram after a subgroup stream.

- A session no longer queues an unlimited number of control messages while the application handles them slower than they arrive.

- `setVideo`, `setAudio` and `setVideoEncoding` called while a `setVideo(false)` or `setAudio(false)` is still pending now apply after it. Before, an encoding change started an encoder for the removed video, and a track set during the removal was dropped from `source.stream` and not encoded again after an unmute.

- An `addSource` still pending when `close()` is called now rejects with "the broadcast is closed" and stops the devices it captured, instead of resolving with a source that kept publishing on the closed broadcast.

- `session.publishTrack(namespace, track, { datagram: true })` sends each object of the track in its own datagram instead of on a subgroup stream: a lost object is not resent and never delays the next one. Keep an object under about 1 KB; one larger than the path allows is dropped. It rejects for a track already written to on streams.

- A FETCH response that arrives after its FETCH was cancelled, as when a
  viewer leaves right after it joins, no longer logs an error, and the
  response stream is stopped instead of being left open.

- A session now closes with PROTOCOL_VIOLATION when the peer sends a datagram or unidirectional stream of an unknown type, or a malformed one, as draft-14 and draft-18 require.

- The automatic quality of `LivePlayer` no longer waits about 13 s to step up when its container grows right after the video is subscribed, or when the catalog adds a rendition above the one playing.

- A live LOC video no longer freezes when the page moves the player's
  container to another element. The browser paused it, and it stayed paused
  until the next resubscribe.

- A seek right after `LivePlayer` switches video rendition no longer fails
  with `Decoding error.`. The player passed the rest of the group before the
  switch to the decoder.

- `parseCatalog` throws on a delta update; `RemoteBroadcast` ignores a catalog `parseCatalog` rejects, and `LivePlayer` reports a catalog error.

- Fixed: on a draft-14 session, a request whose control message is longer than
  65,535 bytes, for example one with a very long authorization token or track
  name, now fails instead of being sent with a truncated length, which broke
  the session.

- On draft-18 sessions, an object, a FETCH object or its properties declared longer than 16 MiB is refused, as on draft-14, instead of being buffered until all of it arrives.

- `addSource()` takes a `codec` per rendition, the source's `encoding.codec` by default, so a source can publish H.265 video with an H.264 rendition for viewers that cannot decode H.265. A rendition the browser cannot encode is reported through `onError` and left out of the catalog.

  ```ts
  await local.addSource('camera', {
    video: true,
    encoding: { codec: 'h265' },
    renditions: [{ name: 'avc', label: '720p', width: 1280, height: 720, bitrate: 2_000_000, codec: 'h264' }]
  })
  ```

- `LivePlayer` picks video only from the tracks the browser can decode, and of renditions with the same size in one switching set it keeps the one with the lowest bitrate. Players of 0.6.0 and earlier do not check the codec and may pick a rendition they cannot decode.

- `LivePlayer` switching to a rendition in another codec, such as from H.265 to H.264, now reconfigures its decoder at the rendition's first keyframe instead of decoding it with the previous codec.

- `LivePlayer.addAudioTrack(name, listener)` plays another live LOC audio
  track of the catalog next to the selected one, on the same clock. It
  starts silent; `onFirstScheduled(dueMs)` tells when it has caught up, and
  the returned `AddedAudioTrack` sets its volume and closes it.

- `LivePlayer.addTextTrack(name, listener)` delivers the objects of a text
  track, such as captions, when the live sound captured with them plays.

- `LivePlayer.setLateSamples('drop' | 'move-clock')` chooses what a sample
  that arrives after its time does, next to `setBufferPolicy` and
  `setCatchUp`.

- `LivePlayer.setVolume(volume, timeConstantSeconds)` takes the time
  constant the live sound approaches the volume with.

- `LivePlayer.stats()` adds `audioDroppedLateSamples`,
  `videoDroppedLateFrames`, `audioCaptureMicros` and `audioObjects`.

- `RemoteBroadcastOptions.keepLiveWhileReviewing` (also an option of
  `watchBroadcasts`) keeps the live subscriptions of every view's player
  forwarding behind a review, as `LivePlayerOptions.keepLiveWhileReviewing`
  does, so going live shows the live picture at once.

- `LivePlayer` asks for a new group once when it switches rendition, returns
  to live or loses its group, instead of repeating the request every 2.2
  seconds. Deploy a relay without the 2-second NEW_GROUP_REQUEST interval
  before upgrading the SDK: an older relay can hold the request back, and the
  picture then waits for the next keyframe.

- `MoqSession.subscribeGroupStart()` sends its joining FETCH after the
  SUBSCRIBE_OK instead of right behind the SUBSCRIBE, so it no longer sends a
  FETCH it then cancels. A join that reads the current group takes one more
  round trip.

- A published video starts a new group on a viewer's request
  (NEW_GROUP_REQUEST) only once its current group is 2 seconds old or holds
  60 objects. A request that arrives earlier is answered when the group
  reaches either.

- Stepping a paused review no longer waits for a FETCH each time:
  `LivePlayer` keeps each group a seek landed on, and the two groups ahead of
  the last move, until it goes live, the tracks change or it stops.

- `LivePlayer.state.seek.shownSeconds` is the capture time of the review
  frame on screen, and undefined while the frame of a seek is being fetched.

- Fixed: going live from a LOC review showed, for a moment, the frame the
  live picture held when it was paused.

- Fixed: a live LOC picture without sound waited 400 ms after a pause before
  it showed a frame again.

- When a source's publish queue overflows and drops video, the source is asked for a keyframe, so viewers get video back as soon as the queue drains instead of at the next periodic keyframe, up to 30 seconds later.

- `LivePlayer` asks the publisher for a new group (NEW_GROUP_REQUEST) when its video decoder fails or loses its reference frames, so the picture comes back without waiting for the next keyframe.

- `addSource` takes `encoding.codec: 'h265'` (Main Profile), or an `hvc1.*` / `hev1.*` codec string. H.265 needs a browser with a hardware encoder for it, such as Chrome on a Mac.

- `LivePlayer` plays an H.265 track where the browser has a hardware decoder for it; other browsers show no picture.

## 0.6.0

- Breaking: the video, each rendition and the audio of a source are encoded
  and sent only while a subscriber takes them, so `stats()` reports 0 for a
  track nobody watches. A track taken up again starts with a keyframe. Deploy
  a relay that turns on a track published in Forward State 0 before upgrading
  the SDK; with an older relay no video or audio is sent.

- Breaking: `MoqSubscription.endBeforeGroup(groupId)` no longer takes the
  Forward State, which the subscription now keeps. Drop the second argument:

  ```ts
  // before
  await subscription.endBeforeGroup(groupId, forward)
  // after
  await subscription.endBeforeGroup(groupId)
  ```

- `LivePlayer.setVideoPriority(priority)` sets the Subscriber Priority of the
  video a player receives: on a congested downlink the relay sends the video
  of a lower value first. It is an integer from 0 to 255, 128 by default; the
  audio stays at 0, ahead of every video. A `SourceView` sets it through
  `view.player`.

  ```ts
  remote.listen({
    onSourceAdd: (view) => {
      if (view.name === 'screen') view.player.setVideoPriority(64)
    }
  })
  ```

- `MoqSubscription.setPriority(priority)` changes the Subscriber Priority of
  a subscription, and `MoqSession.subscribeGroupStart()` takes one as its
  fourth argument.

- `addSource` takes `encoding.latencyMode` (`'realtime'` by default), which
  applies to the video and its renditions. `'quality'` can be set again, for
  example for text on a shared screen.

- `LivePlayerOptions.latencyTolerance` and `LivePlayer.setLatencyTolerance()`
  set how much the live video may fall behind before the automatic quality
  steps down: `'low'` (400 ms, the default), `'high'` (2 s), or milliseconds.
  A `SourceView` sets it through `view.player`.

  ```ts
  remote.listen({
    onSourceAdd: (view) => {
      if (view.name === 'screen') view.player.setLatencyTolerance('high')
    }
  })
  ```

- The `adaptive quality:` log names the reason of the switch (`height`,
  `raise`, `congested` or `calm`) with the picture height, lag and throughput
  it was decided on.

- The automatic quality no longer steps down while a picture stands still. A
  shared screen sends no frames while the screen does not change, and each
  pause dropped it to its lowest rendition, where it stayed until the screen
  kept moving for 10 s.

- A source with renditions keeps publishing when its video and audio start
  within a few frames of each other. Its video and audio could stop with
  `media publishing stopped: group ... precedes the next group ...` while the
  renditions kept flowing.
- `LivePlayer` logs at debug instead of warning that the catalog codec
  changed after the decoder started, when it switches to a rendition whose
  codec string differs only in its level, such as `avc1.64001E` and
  `avc1.64000B`.

- `LivePlayer` and `RemoteBroadcast` start a live broadcast with audio as soon
  as the audio output reports its latency. Playback waited 400 ms before it
  presented anything. A picture without audio, and a browser without
  `AudioContext.outputLatency`, still wait 400 ms.

- Under `Auto`, `LivePlayer` waits for its container to have a height before
  it subscribes to the video, and starts on the rendition that fits it. A
  player of a `RemoteBroadcast` source started on the top rendition and
  switched down about 80 ms later, so the first picture waited for a keyframe
  of the top rendition. A container that never gets a height, such as one the
  page never places, subscribes to no video.

## 0.5.0

- Breaking: `addSource()` takes the video's encoder settings in `encoding`,
  shared by the video and its renditions (`codec`,
  `keyframeIntervalSeconds`, `hardwareAcceleration`), and `main`, the track
  `<source>` (`width`, `height`, `bitrate`, `framerate`), instead of
  `videoEncoderConfig` and `keyframeIntervalFrames`. The keyframe interval is
  in seconds of the frames' timestamps instead of in captured frames.
  `LocalSource.setVideoEncoderConfig()` is
  `setVideoEncoding({ encoding, main })`. Other `VideoEncoderConfig` fields
  are no longer taken: the SDK always encodes with `latencyMode: 'realtime'`
  and `scalabilityMode: 'L1T1'`.

  ```ts
  // Before
  addSource('camera', {
    keyframeIntervalFrames: 60,
    videoEncoderConfig: { codec, width, height, bitrate, framerate: 30 }
  })
  // After
  addSource('camera', {
    encoding: { codec, keyframeIntervalSeconds: 2 },
    main: { width, height, bitrate, framerate: 30 }
  })
  ```

- `LivePlayer`'s automatic quality compares a rendition's height with the
  container's height in CSS pixels instead of device pixels, so a small tile
  on a high-density screen takes a lower rendition: a 215 px tall tile at a
  device pixel ratio of 2 takes 240p instead of 480p.
- `main.framerate` and the new `Rendition.framerate` cap the frames a track
  encodes; `videoEncoderConfig.framerate` was only the encoder's estimate.
  The encoder worker skips frames that come faster, except keyframes, so
  every rendition still starts its groups on the same frames. A rendition
  takes the main video's frame rate unless it sets its own. Without
  `framerate` every frame is encoded, as before.
- A rendition takes `scaleResolutionDownBy` in place of `width` and
  `height`: it divides the captured frame's size, as in WebRTC, so a page
  asks for a quarter of the camera without knowing its size. `addSource()`
  rejects a value below 1 with a `RangeError`.
- The catalog lists a video track's `framerate` only when `main.framerate`
  or the rendition's `framerate` caps it, at that value, instead of 30 for
  every track.
- `encoding.codec` takes a name, `'h264'`, `'vp8'`, `'vp9'` or `'av1'`, and
  the SDK picks the profile; a WebCodecs codec string still picks it
  yourself.
- `encoding.hardwareAcceleration` defaults to `'no-preference'` instead of
  `'prefer-hardware'`. Chrome tries only a hardware encoder for
  `'prefer-hardware'`, so on a machine without a hardware H.264 encoder the
  video failed to start with a `configError`.
- `LivePlayer`'s automatic quality follows a resized tile again as soon as
  the rendition it switched to plays, instead of up to 30 seconds later at
  the next group.

## 0.4.4

- `addSource()` encodes a keyframe every 30 seconds by default instead of
  every 60 frames, and encodes one when a subscriber asks for a new group
  (draft-18 NEW_GROUP_REQUEST). `LivePlayer` asks for one when it switches
  rendition or goes live, and when it joins a broadcast whose current group
  is 60 objects or longer. A viewer on 0.4.3 or older, or on a draft-14
  session, does not ask: set `keyframeIntervalFrames: 60` to keep the old
  interval while such viewers remain.
- `MoqSession.subscribeGroupStart()` subscribes from the first object of a
  group and asks the publisher for a new group when the current one is
  `newGroupAfter` objects or longer. `MoqSubscription.requestNewGroup()` asks
  for a new group on an open subscription.
- The SDK's logs that belong to a session (playback, publishing, encoders,
  session setup and close) reach the telemetry sink with that session's
  token.
- `LivePlayer` changes rendition automatically for a viewer that subscribed
  to a video before the catalog listed its renditions.

## 0.4.3

- `LivePlayer` no longer opens a live broadcast with audio about the audio
  output latency (around 190 ms in Chrome) later than its buffer target for
  the first 10 s. The audio held through the warm-up and due within the
  output latency was scheduled late and pushed the playout clock back.
- A `RemoteBroadcast` view that is told of several visibility changes at once,
  as on a busy page, follows the latest instead of the oldest, so a view on
  screen no longer keeps its video unsubscribed.

## 0.4.2

- `MoqSession.connect()` without `versions` offers draft-18, then draft-14,
  instead of draft-14 only, so a relay that accepts draft-18 is reached over
  it. Pass `versions: ['draft-14']` to stay on draft-14.
- `addSource()` raises the H.264 level of `videoEncoderConfig.codec` to the
  lowest one that holds the encoded size and frame rate. The default
  `avc1.640028` (level 4.0) holds at most 1920x1088 at 30 fps, so since 0.4.1
  a 1920x1200 or larger screen share failed to start with a `configError`.
- The `onError` of a video `configError` names the codec, size and frame
  rate the browser refused, and its error.

## 0.4.1

- `addSource()` encodes each frame at its own size unless `width` and
  `height` in `videoEncoderConfig` bound it, instead of at most 1280x720, so a
  screen share that sets only the bitrate and frame rate is no longer shrunk
  to 720p. A page that wants 720p sets `width` and `height`.
- A new video encoder encodes one frame and drops it before sending live
  frames. Chrome on macOS takes seconds to make the first encoder ready, and
  the frames captured meanwhile were sent together seconds late, so the
  receiver froze on the first frame and widened its buffer to the delay.
- `watchBroadcasts()` and `MoqSession.watchNamespaces()` over draft-18 ask the
  relay for the namespaces only, not for PUBLISH of their tracks, which the
  session refused.
- `init({ telemetry: { stats } })` sends the player stats it returns with each
  report, as `TelemetryStatsSample`s, and `telemetry.url` may be a function
  called at each send. `reportLog()` hands a page's own log to `onLog` and
  `telemetry`.

## 0.4.0

- Breaking: `createBroadcast()` replaces `publish()`, which is removed with
  `Publication`, `PublishOptions` and `SourcePublication`. It announces a
  namespace, and `addSource(name, input)` publishes each source, a camera
  included, in the tracks `name`, `name_audio` and `name_<rendition>`, where
  `publish()` named the first source's tracks `video` and `audio`. A source
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
- `stats()` on each source returns the video and audio that source's
  encoders output over the last second
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
- `addSource()` passes every field of `videoEncoderConfig` to the encoder and
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

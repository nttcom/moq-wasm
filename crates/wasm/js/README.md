# moq-wasm

Media over QUIC Transport in the browser: a `MoqSession` to a relay,
`createBroadcast()` to publish sources, and `watchBroadcasts()` /
`RemoteBroadcast` / `LivePlayer` for what others publish.
`docs/architecture/wasm/browser-sdk.md` is the design.

```ts
import { MoqSession } from 'moq-wasm'
import { createBroadcast, watchBroadcasts } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'

const session = await MoqSession.connect('https://relay.example:4433', { token })
const audioContext = new AudioContext()

joinButton.onclick = async () => {
  await audioContext.resume()
  const local = await createBroadcast(session, ['room', 'alice'])
  const camera = await local.addSource('camera', { video: true, audio: true })
  preview.srcObject = camera.stream

  await watchBroadcasts(session, ['room'], {
    audioContext,
    onAdd: (remote) => remote.listen({ onSourceAdd: (view) => document.querySelector('#grid')!.append(view.element) }),
    onRemove: () => {}
  })
}
```

`moq-wasm/media` stays below 1.0 whatever the package version: any minor
version may change it.

| Entry point                   | Contents                                                                                                                         |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `moq-wasm`                    | `MoqSession` and the track-level API: subscribe, fetch, publish tracks, watch namespaces                                         |
| `moq-wasm/media`              | `createBroadcast()`, `watchBroadcasts()`, `RemoteBroadcast`, `LivePlayer`, `PlayerControls`, `PlaybackGroup` and the MSF catalog |
| `moq-wasm/recording`          | `openRecording()`, which has `services/recorder` replay a recording                                                              |
| `moq-wasm/media/player.css`   | Styles of `LivePlayer`'s picture                                                                                                 |
| `moq-wasm/media/controls.css` | Styles of `PlayerControls`                                                                                                       |

## Installing

The package is not on the npm registry. Each release attaches its tarball to
the GitHub Release `sdk-v<version>`; put its URL in `package.json`:

```json
{
  "dependencies": {
    "moq-wasm": "https://github.com/nttcom/moq-wasm/releases/download/sdk-v0.3.0/moq-wasm-0.3.0.tgz"
  }
}
```

The tarball holds the built JavaScript, types, wasm and workers, so neither
Rust nor wasm-pack is needed. Update by changing the URL. With Vite, see
[Vite](#vite) below.

## Connecting

```ts
import { MoqSession, RequestError } from 'moq-wasm'

const session = await MoqSession.connect('https://relay.example:4433', {
  // A function is called before each CLIENT_SETUP, a reconnect or a GOAWAY move included.
  token: () => fetchTokenFromMyBackend(),
  // The MoQT drafts offered, most preferred first; draft-18, then draft-14, when omitted.
  versions: ['draft-18', 'draft-14']
})
session.closed.then(({ code, reason }) => console.log('session closed', code, reason))

// ...
session.close()
```

`connect` loads the wasm module on its first call. A request the relay rejects
throws a `RequestError` whose `errorCode` is the draft-14 error code
(`RequestErrorCode`) and whose `reasonPhrase` is the relay's reason.

`session.transportStats()` returns the bytes the session has sent and
received since it opened, across GOAWAY moves and reconnects. Sample it twice for a send and receive
rate, for example next to a source's `encodedKbps`:

```ts
const before = session.transportStats()
await new Promise((resolve) => setTimeout(resolve, 1000))
const after = session.transportStats()
const sentKbps = ((after.sentBytes - before.sentBytes) * 8) / 1000
```

The SDK counts them itself, since Chrome has no `WebTransport.getStats()`:

- `sentBytes` counts a write once the browser accepted it, not once the relay
  acknowledged it. The browser accepts writes more slowly while its send
  buffer is full, so a sending rate below `encodedKbps` means the connection
  cannot keep up.
- Over WebTransport they are the bytes of streams and datagrams, without QUIC
  and HTTP/3 framing; over the WebSocket fallback, the QMux frames, without
  WebSocket and TLS framing.
- They say nothing about round-trip time or loss.

## Publishing: `createBroadcast`

`createBroadcast()` announces a namespace at once. Each `addSource(name)` publishes a
source in the tracks `name`, `name_audio` and `name_<rendition>`, listed in the
broadcast's one MSF catalog. The video is H.264 by default and the audio Opus.

```ts
import { createBroadcast } from 'moq-wasm/media'

const local = await createBroadcast(session, ['room', 'alice'], { onError: console.error })

// The SDK captures the devices: true, or constraints such as a deviceId.
const camera = await local.addSource('camera', {
  // The label of the video, and of the audio followed by " audio".
  label: 'Camera',
  video: { deviceId: cameraId },
  audio: true,
  // Shared by the video and its renditions. Defaults: H.264, a keyframe every
  // 30 seconds on the same frames in every track.
  // `codec` is 'h264', 'vp8', 'vp9' or 'av1', or a WebCodecs codec string.
  // `latencyMode: 'quality'` trades latency for a sharper picture.
  encoding: { codec: 'av1', keyframeIntervalSeconds: 10 },
  // The track `camera`. Defaults: 1 Mbps, every frame at its own size;
  // `width` and `height` bound the size, `framerate` skips faster frames.
  main: { width: 1920, height: 1080, bitrate: 2_500_000, framerate: 30 },
  // Each rendition is the track `camera_<name>`, encoded from the same frames
  // with the same keyframes. Its size is bounded by `width` and `height`, or
  // the captured frame's divided by `scaleResolutionDownBy`. `framerate`
  // defaults to the main video's.
  renditions: [
    { name: '360p', label: '360p', width: 640, height: 360, bitrate: 600_000 },
    { name: 'low', label: 'Low', scaleResolutionDownBy: 4, bitrate: 150_000, framerate: 15 }
  ]
})
preview.srcObject = camera.stream // the tracks the source publishes

await camera.setVideo({ deviceId: otherCameraId }) // switch the device
await camera.setVideoEncoding({ encoding: { codec: 'vp8' }, main: { bitrate: 1_000_000 } })
await camera.setMuted('audio', true) // stop sending, keep the device
await camera.setVideo(false) // stop sending and release the device

// A track the page processed itself, such as a blurred camera; the SDK does not stop it.
await camera.setVideo(blurredTrack)

// The screen, from a user gesture. The browser's "Stop sharing" removes the source.
const screen = await local.addSource('screen', { display: { audio: true } })
screen.listen({ onEnded: () => showShareButton() })

await local.removeSource('screen')
await local.close() // PUBLISH_DONE on every track; the session stays open
```

`video` and `audio` take `true` or `MediaTrackConstraints`, which the SDK captures
in one `getUserMedia` call, or a `MediaStreamTrack` of the page; `stream` takes the
first video and audio tracks of a `MediaStream`. The SDK stops only the tracks it
captured. A refused permission rejects with the `getUserMedia` error.

`stats()` on each source returns what that
source's encoders output over the last second, `encodedKbps` and
`encodedFramesPerSecond`, and their `encoderErrors` since it was published.
`video` also has the encoded `frameSize`, the `droppedFrames` the encoder
worker closed before encoding because an encoder's queue was full or no
encoder could be configured, and each rendition by name in `renditions`. A
kind the source has no track of is `undefined`.

```ts
const { video, audio } = screen.stats()
console.log(video?.encodedKbps, video?.frameSize, video?.droppedFrames, audio?.encoderErrors)
```

## Playing the people in a room: `watchBroadcasts` and `RemoteBroadcast`

`watchBroadcasts()` plays every broadcast published under a prefix, those published
before the call included, except the ones the session published itself. Each is a
`RemoteBroadcast`, which subscribes to its catalog once and creates a view per
source the catalog lists.

```ts
import { watchBroadcasts } from 'moq-wasm/media'

const room = await watchBroadcasts(session, ['room'], {
  filter: (namespace) => namespace.at(-1) !== 'transcriber',
  onAdd: (remote) => {
    const tile = newTile(remote.namespace.at(-1)!) // the page's own element
    remote.listen({
      // Called for the sources there are when `listen` is called too.
      onSourceAdd: (view) => {
        tile.append(view.element) // created and removed by the SDK
        view.listen({ onChange: () => tile.show(view.name, view.state) })
      },
      onAudioLevel: (view, level) => tile.speaking(level)
    })
  },
  onRemove: (remote) => removeTile(remote.namespace.at(-1)!)
})
// ...
await room.close()
```

- `view.state` is `{ video: 'waiting' | 'playing' | 'off', audio: 'playing' | 'off', videoSize? }`,
  also on `view.element` as `data-video` and `data-audio`. A camera whose video is
  muted keeps its view, with `video: 'off'`.
- `view.element` fills the box the page gives it; `--moq-object-fit` sets how the
  picture fits (`contain` by default). The page may move it to another parent within
  one task without the picture stopping.
- A view subscribes its video only while its element is on screen, and keeps the
  audio; `{ pauseHiddenVideo: false }` turns that off.
- `view.setVolume()`, `view.stats()` and `view.setVideoEnabled()` act on the source;
  `view.player` is its `LivePlayer` for the rest.

A page that shows one broadcast uses `new RemoteBroadcast(session, namespace)` directly.

## Audio output

Pass the page's `AudioContext` as `audioContext` to `watchBroadcasts` or
`RemoteBroadcast`, as to a `LivePlayer`: every view plays its live LOC sound
through it. Resume it from a user gesture, such as the join button, so that the
views of someone who joins after the click are heard too, and pick the speaker
with `audioContext.setSinkId(deviceId)`. TypeScript's DOM types (5.9 and 7.0)
do not declare `setSinkId` on `AudioContext`, so declare it in one of the
page's modules:

```ts
declare global {
  interface AudioContext {
    setSinkId(sinkId: string): Promise<void>
  }
}
```

## Playing a broadcast: `LivePlayer`

`LivePlayer` subscribes to the catalog of a namespace, picks its video and
audio tracks, and plays them in `container`. It plays a live broadcast with
rewind, and a broadcast none of whose tracks is live as VOD.

```ts
import { LivePlayer, PlayerControls } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

const container = document.querySelector<HTMLElement>('#stage')!
const player = new LivePlayer({
  container,
  callbacks: {
    // The state changed: tracks, mode, pause, status texts.
    onStateChange: () => controls.render(),
    // A live video frame was shown.
    onLiveFrame: () => controls.renderBuffer(player.stats()),
    onLog: (level, message) => console.log(level, message)
  }
})
// Seek bar, play/pause, LIVE, volume and quality menu.
const controls = new PlayerControls(container, player, (level, message) => console.log(level, message))

watchButton.onclick = () => player.start(session, ['room', 'alice'])
```

Call `start` from a click, or `unlockAudio()` before: the browser's autoplay
policy keeps an AudioContext opened without a user gesture silent. `start`
resolves once the tracks are subscribed or, when nobody publishes the namespace
yet, once the player waits for the publisher;
it starts playing when the publisher appears. `stop()` unsubscribes; the
session stays the caller's to close. `dispose()` also releases the player's
decoder workers, audio contexts and pictures, for a page that creates a player
per broadcast; it cannot start again.

Without `PlayerControls`, draw the controls from `player.state` in
`onStateChange` and drive the player with its methods:

| Need                          | API                                                                                                                                                                         |
| ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Tracks and the current choice | `state.videoTracks`, `state.audioTracks`, `state.selectedVideoTrack`, `state.selectedAudioTrack`                                                                            |
| Pick a track                  | `selectVideoTrack(name)`, `selectAudioTrack(name)`; `setAdaptiveVideo(true)` chooses the rendition by itself                                                                |
| Live or rewound               | `state.mode` (`'live'` / `'review'`), `goLive()`                                                                                                                            |
| Seek                          | `seek(captureMicros)` to a LOC capture timestamp, `skip(seconds)`; `state.seek` holds the seekable range in capture seconds (`replayableStartSeconds` to `liveEdgeSeconds`) |
| Pause, volume, speed          | `setPaused(paused)`, `setVolume(volume)`, `setPlaybackRate(rate)`                                                                                                           |
| Hide the picture              | `setVideoEnabled(false)` unsubscribes the video and keeps the audio; `setVideoEnabled(true)` subscribes it again                                                            |
| Video ahead of other videos   | `setVideoPriority(priority)`: on a congested downlink the relay sends the video of a lower value first (0 to 255, 128 by default); the audio stays ahead of every video     |
| Status texts                  | `state.catalogStatus`, `state.playbackStatus`, `state.rewindStatus`                                                                                                         |
| Delivery statistics           | `stats()`, `formatLiveStats(stats)`                                                                                                                                         |

Options of `new LivePlayer`:

| Option                   | Effect                                                                                                                                                                                                                                                           |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `source`                 | Plays only this source of the catalog, `{ video: 'screen', audio: 'screen_audio' }`; without it every video and audio track is a choice                                                                                                                          |
| `keepLiveWhileReviewing` | Keeps the live tracks playing behind a rewind, so `goLive()` shows the live picture at once, at the cost of the live bandwidth and decoding                                                                                                                      |
| `callbacks.onAudioLevel` | Receives the LOC Audio Level of each live audio object that carries one                                                                                                                                                                                          |
| `audioContext`           | Plays the live LOC sound through the page's context; see below                                                                                                                                                                                                   |
| `latencyTolerance`       | How much the live video may fall behind before the automatic quality steps down: `'low'` (400 ms, the default), `'high'` (2 s), or milliseconds, `Infinity` for never; `setLatencyTolerance()` changes it. A shared screen keeps its text readable with `'high'` |

Players on one page can share one `AudioContext` through `audioContext`, so
they mix into one output stream and the page picks the speaker with
`audioContext.setSinkId(deviceId)`. The context stays the page's: a player
disconnects its nodes from it and never suspends or closes it, so resume it
after a user gesture and close it when the page is done. A stream at another
sample rate is resampled by the context. Only the live LOC sound plays through
it: the sound of a rewind or a recording (`state.mode === 'review'`, a
`PlaybackGroup`) plays through a context the player opens, and CMAF plays
through a `<video>` element, so neither follows the context's `setSinkId`.

```ts
const audioContext = new AudioContext()
const players = members.map((member) => new LivePlayer({ container: member.view, callbacks, audioContext }))
joinButton.onclick = () => void audioContext.resume()
speakerSelect.onchange = () => void audioContext.setSinkId(speakerSelect.value)
```

To play tracks the page already knows instead of the catalog's, pass them as
the third argument of `start`: `player.start(session, namespace, { video: [track], audio: [track] })`.
Either list may be left out, but not both.

A broadcast with audio and no video, such as a member who joined with the
camera off, plays its sound live, whether its catalog, its `source` or the
given tracks list no video: `state.videoTracks` is empty, `state.playbackStatus`
reads `Playing <audio track>`, and the PUBLISH_DONE of the audio takes given
tracks back to waiting for the publisher. It cannot be rewound or played as
VOD: the seek axis follows the video's groups, so `state.seek.seekable` stays
false and `seek` and `skip` do nothing. Live CMAF also needs a video track.

Players that play sources of one broadcast share its catalog through a
`BroadcastCatalog`, since one session must not subscribe to a track twice
(draft-ietf-moq-transport-14 §9.7). Pass it as the third argument of `start`;
the page can read the catalog from it too:

```ts
const catalog = new BroadcastCatalog(session, namespace)
const unlisten = catalog.listen({ onCatalog: (text) => layout(parseCatalog(text)), onPublisherAbsent: () => {} })
await camera.start(session, namespace, catalog)
await screen.start(session, namespace, catalog)
```

`stop()` leaves a shared catalog open; `catalog.close()` unsubscribes it.

Tell a stopped track from a departed publisher by the catalog: a track that
a publisher mutes or whose source it closes leaves the catalog, and the
namespace of a publisher whose session drops is withdrawn. PUBLISH_DONE does
not tell them apart: both end the track's subscriptions with `TRACK_ENDED`
(draft-ietf-moq-transport-14 §9.12).

## Playing several broadcasts together: `PlaybackGroup`

`PlaybackGroup` plays its players at one position on one clock. While live,
each player plays at its own live edge; a rewind or VOD aligns them on the LOC
timestamp. A player's own `seek`, `skip`, `goLive` and `setPaused` act on its
whole group.

```ts
import { PlaybackGroup } from 'moq-wasm/media'

const group = new PlaybackGroup({ onStateChange: () => render(group.state) })
group.add(alicePlayer)
group.add(bobPlayer)
group.seek(captureMicros) // or play(), pause(), skip(seconds), goLive()
```

`group.state` holds `mode`, `positionMicros`, `paused` and `ended`. A player
whose range does not cover the position fetches nothing and reports
`state.inRange` false.

## Playing a recording

`openRecording()` asks `services/recorder` to replay a finished recording to
the relay `session` is connected to, and keeps the replay running until
`close()`. The replay publishes each recorded broadcast under
`recording.prefix`; play them with `LivePlayer`s, which see them as VOD.

```ts
import { PlaybackGroup, LivePlayer } from 'moq-wasm/media'
import { openRecording } from 'moq-wasm/recording'

const recording = await openRecording(session, 'https://recorder.example', recordingId, { token })
const group = new PlaybackGroup()
for await (const event of await session.watchNamespaces(recording.prefix)) {
  if (event.kind === 'announced') {
    const player = new LivePlayer({ container: newTile(), callbacks })
    group.add(player)
    await player.start(session, event.namespace)
  }
}
// ...
recording.close()
```

`recording.startedAtMs` is the Unix time the recording started; a replay's LOC
timestamps count media time from it. A refused request rejects with a
`RecorderError` carrying the HTTP `status`.

## Tracks without media

`MoqSession` publishes and subscribes to tracks of any payload, such as chat or
control messages.

```ts
// Publisher
const announcement = await session.announce(['room', 'alice'])
const chat = await session.publishTrack(['room', 'alice'], 'chat')
await chat.writeGroup(new TextEncoder().encode('hello')) // one object as a group of its own
await chat.write(payload, true, undefined) // starts a new group; `false` adds to the current one
await announcement.close() // PUBLISH_NAMESPACE_DONE

// Subscriber
const subscription = await session.subscribe(['room', 'alice'], 'chat')
for await (const object of subscription) {
  console.log(object.groupId, new TextDecoder().decode(object.objectPayload))
}
await subscription.close() // UNSUBSCRIBE

// Past objects: FETCH from a start location to an end location (exclusive)
const largest = await session.trackStatus(['room', 'alice'], 'chat')
const past = await session.fetch(
  ['room', 'alice'],
  'chat',
  { groupId: 0n, objectId: 0n },
  { groupId: largest!.groupId + 1n, objectId: 0n }
)
for await (const object of past) {
  /* ... */
}

// Namespaces published under a prefix, those published before included
for await (const event of await session.watchNamespaces(['room'])) {
  console.log(event.kind, event.namespace) // 'announced' or 'withdrawn'
}
```

`publishTrack` keeps each object for 60 s to answer FETCH (`retentionMs`
changes it). `chat.nextSubscriber()` resolves when a new subscriber arrives:
its SUBSCRIBE starts after the newest object, so send it what it needs then.
`subscription.groups()` reads each subgroup stream on its own, and
`subscription.done` resolves once the publisher ends the subscription.

## Logs

The SDK's logs are discarded unless the page installs a sink before
connecting:

```ts
import { init } from 'moq-wasm'

await init({
  logLevel: 'info',
  onLog: (record) => console.log(record.level, record.target, record.message, record.fields)
})
```

`onSpan` receives the wasm module's spans, and `telemetry` sends both to an
observability server every 10 s, with the player stats its `stats` returns:

```ts
import { init, reportLog } from 'moq-wasm'

await init({
  logLevel: 'info',
  telemetry: {
    url: 'https://observability.example', // or a function, called at each send
    page: 'viewer',
    stats: () => (player.state.started ? [{ values: player.stats() }] : [])
  }
})

// The page's own logs are sent only when handed over
reportLog('warn', 'viewer', 'stalled', { track_name: 'video' })
```

A page playing several tracks sets `trackNamespace` (joined with `/`) and
`mediaSource` on each sample.

## Vite

The package loads its `.wasm` file and its workers with
`new URL(..., import.meta.url)`. The Vite dev server's dependency pre-bundling
moves the code to `node_modules/.vite/deps` without those files, so the URLs
break. Exclude the package from it:

```js
// vite.config.js
export default { optimizeDeps: { exclude: ['moq-wasm'] } }
```

`vite build` needs nothing.

## Building

```shell
npm --prefix crates/wasm/js run wasm    # wasm-pack into pkg/
npm --prefix crates/wasm/js run build   # dist/ and types/
npm --prefix crates/wasm/js run check:pack
```

`check:pack` packs the package and builds a new Vite project from the tarball.
Inside this repository, `examples/browser` reads the TypeScript sources through
the `source` export condition instead.

## Releasing

Raise `version` in `package.json`, add its section to `CHANGELOG.md`, and push
a `sdk-v<version>` tag. `.github/workflows/sdk-release.yml` builds the package
and attaches the `npm pack` tarball to a GitHub Release of the same name.

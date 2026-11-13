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

| Entry point                   | Contents                                                                                                     |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `moq-wasm`                    | `MoqSession` and the track-level API: subscribe, fetch, publish tracks, watch namespaces                     |
| `moq-wasm/media`              | `createBroadcast()`, `watchBroadcasts()`, `RemoteBroadcast`, `LivePlayer`, `PlayerControls`, `PlaybackGroup` |
| `moq-wasm/recording`          | `openRecording()`, which has `services/recorder` replay a recording                                          |
| `moq-wasm/media/player.css`   | Styles of `LivePlayer`'s picture                                                                             |
| `moq-wasm/media/controls.css` | Styles of `PlayerControls`                                                                                   |

`moq-wasm/media` stays below 1.0 whatever the package version: any minor
version may change it.

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
Rust nor wasm-pack is needed.

With Vite, exclude the package from the dev server's dependency pre-bundling,
which loses the `.wasm` file and the workers (`vite build` needs nothing):

```js
// vite.config.js
export default { optimizeDeps: { exclude: ['moq-wasm'] } }
```

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

- A request the relay rejects throws a `RequestError` with the draft's
  `errorCode` (`RequestErrorCode`) and the relay's `reasonPhrase`.
- `session.transportStats()` returns the `sentBytes` and `receivedBytes` of
  the session since it opened; sample it twice for a rate. A sending rate
  below a source's `encodedKbps` means the connection cannot keep up.

## Publishing: `createBroadcast`

`createBroadcast()` announces a namespace. Each `addSource(name)` publishes a
source as the tracks `name`, `name_audio` and `name_<rendition>`, listed in
the broadcast's MSF catalog. The video is H.264 by default and the audio Opus.

```ts
import { createBroadcast } from 'moq-wasm/media'

const local = await createBroadcast(session, ['room', 'alice'], { onError: console.error })

const camera = await local.addSource('camera', {
  label: 'Camera',
  video: { deviceId: cameraId }, // true, constraints or a MediaStreamTrack of the page
  audio: true,
  // Shared by the video and its renditions. `codec` is 'h264', 'h265', 'vp8',
  // 'vp9' or 'av1'; a keyframe every 30 seconds by default.
  encoding: { codec: 'av1', keyframeIntervalSeconds: 10 },
  // The track `camera`. Defaults: 1 Mbps, every frame at its own size.
  main: { width: 1920, height: 1080, bitrate: 2_500_000, framerate: 30 },
  // Each rendition is the track `camera_<name>`, with the same keyframes. A
  // viewer picks among those it can decode, so an H.264 rendition serves
  // viewers without AV1.
  renditions: [
    { name: '360p', label: '360p', width: 640, height: 360, bitrate: 600_000 },
    { name: 'low', label: 'Low', scaleResolutionDownBy: 4, bitrate: 150_000, framerate: 15 },
    { name: '360p_avc', label: '360p', width: 640, height: 360, bitrate: 800_000, codec: 'h264' }
  ]
})
preview.srcObject = camera.stream // the tracks the source publishes

await camera.setVideo({ deviceId: otherCameraId }) // switch the device
await camera.setVideoEncoding({ encoding: { codec: 'vp8' }, main: { bitrate: 1_000_000 } })
await camera.setMuted('audio', true) // stop sending, keep the device
await camera.setVideo(false) // stop sending and release the device

// The screen, from a user gesture. The browser's "Stop sharing" removes the source.
const screen = await local.addSource('screen', { display: { audio: true } })
screen.listen({ onEnded: () => showShareButton() })

await local.removeSource('screen')
await local.close() // PUBLISH_DONE on every track; the session stays open
```

- The SDK stops only the tracks it captured itself, not a `MediaStreamTrack`
  the page passed. A refused permission rejects with the `getUserMedia` error.
- Each track is encoded and sent only while a subscriber takes it, so
  `stats()` reports 0 for a track nobody watches.
- `source.stats()` returns `encodedKbps`, `encodedFramesPerSecond` and
  `encoderErrors` for `video` and `audio`; `video` also has `frameSize`,
  `droppedFrames` and each rendition by name in `renditions`.

## Playing a room: `watchBroadcasts` and `RemoteBroadcast`

`watchBroadcasts()` plays every broadcast published under a prefix, earlier
ones included, except the session's own. Each is a `RemoteBroadcast`, which
creates a view per source its catalog lists. A page that shows one broadcast
uses `new RemoteBroadcast(session, namespace)` directly.

```ts
import { watchBroadcasts } from 'moq-wasm/media'

const room = await watchBroadcasts(session, ['room'], {
  audioContext,
  filter: (namespace) => namespace.at(-1) !== 'transcriber',
  onAdd: (remote) => {
    const tile = newTile(remote.namespace.at(-1)!) // the page's own element
    remote.listen({
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
  also on `view.element` as `data-video` and `data-audio`.
- `view.element` fills the box the page gives it; `--moq-object-fit` sets how
  the picture fits (`contain` by default).
- A view subscribes its video only while its element is on screen;
  `{ pauseHiddenVideo: false }` turns that off.
- `view.setVolume()`, `view.stats()` and `view.setVideoEnabled()` act on the
  source; `view.player` is its `LivePlayer` for the rest.

### Audio output

Pass the page's `AudioContext` as `audioContext` to `watchBroadcasts`,
`RemoteBroadcast` or `LivePlayer`: the live sound plays through it. Resume it
from a user gesture, such as the join button, and pick the speaker with
`audioContext.setSinkId(deviceId)`. The context stays the page's: the SDK
never suspends or closes it. The sound of a rewind, a recording or CMAF plays
elsewhere and does not follow `setSinkId`. TypeScript's DOM types do not
declare `setSinkId` on `AudioContext`, so declare it in one of the page's
modules:

```ts
declare global {
  interface AudioContext {
    setSinkId(sinkId: string): Promise<void>
  }
}
```

## Playing a broadcast: `LivePlayer`

`LivePlayer` subscribes to the catalog of a namespace, picks its video and
audio tracks, and plays them in `container`: a live broadcast with rewind, or
a broadcast none of whose tracks is live as VOD.

```ts
import { LivePlayer, PlayerControls } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

const container = document.querySelector<HTMLElement>('#stage')!
const player = new LivePlayer({
  container,
  callbacks: {
    onStateChange: () => controls.render(), // tracks, mode, pause, status texts
    onLiveFrame: () => controls.renderBuffer(player.stats()),
    onLog: (level, message) => console.log(level, message)
  }
})
// Seek bar, play/pause, LIVE, volume and quality menu.
const controls = new PlayerControls(container, player, (level, message) => console.log(level, message))

watchButton.onclick = () => player.start(session, ['room', 'alice'])
```

Call `start` from a click, or `unlockAudio()` before: the browser's autoplay
policy keeps an AudioContext opened without a user gesture silent. When
nobody publishes the namespace yet, the player waits for the publisher.
`stop()` unsubscribes; `dispose()` also releases the player's workers and
audio contexts, after which it cannot start again. The session stays the
caller's to close.

Without `PlayerControls`, draw the controls from `player.state` in
`onStateChange`:

| Need                          | API                                                                                                          |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------ |
| Tracks and the current choice | `state.videoTracks`, `state.audioTracks`, `state.selectedVideoTrack`, `state.selectedAudioTrack`             |
| Pick a track                  | `selectVideoTrack(name)`, `selectAudioTrack(name)`; `setAdaptiveVideo(true)` chooses the rendition by itself |
| Live or rewound               | `state.mode` (`'live'` / `'review'`), `goLive()`                                                             |
| Seek                          | `seek(captureMicros)`, `skip(seconds)`; `state.seek` holds the seekable range                                |
| Pause, volume, speed          | `setPaused(paused)`, `setVolume(volume)`, `setPlaybackRate(rate)`                                            |
| Hide the picture              | `setVideoEnabled(false)` unsubscribes the video and keeps the audio                                          |
| Video ahead of other videos   | `setVideoPriority(priority)`: 0 to 255, the lower value first on a congested downlink (128 by default)       |
| Status texts                  | `state.catalogStatus`, `state.playbackStatus`, `state.rewindStatus`                                          |
| Delivery statistics           | `stats()`, `formatLiveStats(stats)`                                                                          |

Options of `new LivePlayer`:

| Option                   | Effect                                                                                                                                        |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------- |
| `source`                 | Plays only this source of the catalog, `{ video: 'screen', audio: 'screen_audio' }`                                                           |
| `keepLiveWhileReviewing` | Keeps the live tracks playing behind a rewind, so `goLive()` shows the live picture at once                                                   |
| `callbacks.onAudioLevel` | Receives the LOC Audio Level of each live audio object that carries one                                                                       |
| `audioContext`           | Plays the live sound through the page's context (see [Audio output](#audio-output))                                                           |
| `latencyTolerance`       | How far the live video may fall behind before the automatic quality steps down: `'low'` (400 ms, the default), `'high'` (2 s) or milliseconds |

- `player.start(session, namespace, { video: [track], audio: [track] })` plays
  tracks the page already knows instead of the catalog's.
- A broadcast with audio and no video plays live, but cannot be rewound or
  played as VOD.
- Players of several sources of one broadcast share its catalog through a
  `BroadcastCatalog`, passed as the third argument of `start`, since one
  session must not subscribe to a track twice:

  ```ts
  const catalog = new BroadcastCatalog(session, namespace)
  await camera.start(session, namespace, catalog)
  await screen.start(session, namespace, catalog)
  ```

- Tell a stopped track from a departed publisher by the catalog: a muted or
  closed track leaves the catalog, while a publisher whose session drops has
  its namespace withdrawn. PUBLISH_DONE is the same for both.

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

## Playing a recording

`openRecording()` asks `services/recorder` to replay a finished recording to
the relay `session` is connected to, until `close()`. The replay publishes
each recorded broadcast under `recording.prefix`; `LivePlayer`s play them as
VOD.

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

A refused request rejects with a `RecorderError` carrying the HTTP `status`.

## Tracks without media

`MoqSession` publishes and subscribes to tracks of any payload, such as chat
or control messages.

```ts
// Publisher
const announcement = await session.announce(['room', 'alice'])
const chat = await session.publishTrack(['room', 'alice'], 'chat')
await chat.writeGroup(new TextEncoder().encode('hello')) // one object as a group of its own
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

// Namespaces published under a prefix, those published before included
for await (const event of await session.watchNamespaces(['room'])) {
  console.log(event.kind, event.namespace) // 'announced' or 'withdrawn'
}
```

- `publishTrack` keeps each object for 60 s to answer FETCH (`retentionMs`
  changes it).
- `chat.nextSubscriber()` resolves when a new subscriber arrives: its
  SUBSCRIBE starts after the newest object, so send it what it needs then.
- `publishTrack(namespace, track, { datagram: true })` sends each object in
  its own datagram: a lost object is not resent and never delays the next
  one. Keep an object under about 1 KB.

## Logs

The SDK's logs are discarded unless the page installs a sink before
connecting. `telemetry` also sends them, with the wasm module's spans and the
player stats its `stats` returns, to an observability server every 10 s:

```ts
import { init, reportLog } from 'moq-wasm'

await init({
  logLevel: 'info',
  onLog: (record) => console.log(record.level, record.target, record.message, record.fields),
  telemetry: {
    url: 'https://observability.example',
    page: 'viewer',
    stats: () => (player.state.started ? [{ values: player.stats() }] : [])
  }
})

// The page's own logs are sent only when handed over
reportLog('warn', 'viewer', 'stalled', { track_name: 'video' })
```

## Building and releasing

```shell
npm --prefix crates/wasm/js run wasm        # wasm-pack into pkg/
npm --prefix crates/wasm/js run build       # dist/ and types/
npm --prefix crates/wasm/js run check:pack  # builds a new Vite project from the tarball
```

To release, follow `.agents/skills/release-sdk/SKILL.md`: a `sdk-v<version>`
tag makes `.github/workflows/sdk-release.yml` attach the tarball to a GitHub
Release.

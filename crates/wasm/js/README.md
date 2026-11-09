# moq-wasm

Media over QUIC Transport in the browser: a `MoqSession` to a relay,
`publish()` for a `MediaStream`, and `LivePlayer` for what others publish.
`docs/architecture/wasm/browser-sdk.md` is the design.

```ts
import { MoqSession } from 'moq-wasm'
import { LivePlayer, publish } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'

const session = await MoqSession.connect('https://relay.example:4433', { token })
await publish(session, ['room', 'alice'], await navigator.mediaDevices.getUserMedia({ video: true, audio: true }))

const player = new LivePlayer({
  container: document.querySelector<HTMLElement>('#bob')!,
  callbacks: { onStateChange() {}, onLiveFrame() {}, onLog: console.log }
})
await player.start(session, ['room', 'bob'])
```

`moq-wasm/media` stays below 1.0 whatever the package version: any minor
version may change it.

| Entry point                   | Contents                                                                                             |
| ----------------------------- | ---------------------------------------------------------------------------------------------------- |
| `moq-wasm`                    | `MoqSession` and the track-level API: subscribe, fetch, publish tracks, watch namespaces             |
| `moq-wasm/media`              | `publish()` for a `MediaStream`, `LivePlayer`, `PlayerControls`, `PlaybackGroup` and the MSF catalog |
| `moq-wasm/recording`          | `openRecording()`, which has `services/recorder` replay a recording                                  |
| `moq-wasm/media/player.css`   | Styles of `LivePlayer`'s picture                                                                     |
| `moq-wasm/media/controls.css` | Styles of `PlayerControls`                                                                           |

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
  // The MoQT drafts offered, most preferred first; draft-14 only when omitted.
  versions: ['draft-18', 'draft-14']
})
session.closed.then(({ code, reason }) => console.log('session closed', code, reason))

// ...
session.close()
```

`connect` loads the wasm module on its first call. A request the relay rejects
throws a `RequestError` whose `errorCode` is the draft-14 error code
(`RequestErrorCode`) and whose `reasonPhrase` is the relay's reason.

## Publishing a camera and microphone

`publish()` encodes the stream's first video track (H.264 by default) and
first audio track (Opus) and publishes them as an MSF broadcast in
`namespace`, with its catalog.

```ts
import { publish } from 'moq-wasm/media'

const stream = await navigator.mediaDevices.getUserMedia({ video: true, audio: true })
const publication = await publish(session, ['room', 'alice'], stream, {
  videoLabel: 'Camera',
  // Replaces the encoder's defaults: H.264, at most 1280x720, 1 Mbps, 30 fps.
  videoEncoderConfig: { codec: 'av01.0.08M.08', bitrate: 2_500_000 },
  // Each rendition is the track `video_<name>`, encoded from the same frames.
  renditions: [{ name: '360p', label: '360p', width: 640, height: 360, bitrate: 600_000 }],
  onError: (error) => console.error(error)
})

await publication.setMuted('audio', true)
publication.replaceTrack('video', otherStream.getVideoTracks()[0])

// A second source in the same broadcast: tracks `screen` and `screen_audio`.
const screen = publication.addSource('screen', await navigator.mediaDevices.getDisplayMedia())

await publication.close() // PUBLISH_DONE on every track; the session stays open
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

Call `start` from a click: the player creates its AudioContext on first use,
and the browser's autoplay policy keeps a context created without a user
gesture silent. `start` resolves once the tracks are subscribed or, when
nobody publishes the namespace yet, once the player waits for the publisher;
it starts playing when the publisher appears. `stop()` unsubscribes; the
session stays the caller's to close.

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
| Status texts                  | `state.catalogStatus`, `state.playbackStatus`, `state.rewindStatus`                                                                                                         |
| Delivery statistics           | `stats()`, `formatLiveStats(stats)`                                                                                                                                         |

Options of `new LivePlayer`:

| Option                   | Effect                                                                                                                                      |
| ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------- |
| `source`                 | Plays only this source of the catalog, `{ video: 'screen', audio: 'screen_audio' }`; without it every video and audio track is a choice     |
| `keepLiveWhileReviewing` | Keeps the live tracks playing behind a rewind, so `goLive()` shows the live picture at once, at the cost of the live bandwidth and decoding |
| `callbacks.onAudioLevel` | Receives the LOC Audio Level of each live audio object that carries one                                                                     |

To play tracks the page already knows instead of the catalog's, pass them as
the third argument of `start`: `player.start(session, namespace, { video: [track], audio: [track] })`.

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
observability server.

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

# moq-wasm for JavaScript

Browser SDK for publishing and playing live video, audio, and data over Media over QUIC Transport (MoQT), using WebTransport (with WebSocket fallback) and WebCodecs.

## Core concepts

- **Session (`MoqSession`)**: A connection to a MoQT relay. Reconnects and `GOAWAY` handoffs are handled automatically.
- **Namespace & track**: Streams live at a `namespace` path (e.g. `['anon', 'room-1']`) and a track name (e.g. `'camera'`). Public demo namespaces start with `'anon'`; authenticated apps use a JWT whose `appId` is the first segment.
- **Broadcast (MSF)**: A bundle of video, audio, and event tracks described by a `catalog` track, interoperable with the [Python](https://github.com/nttcom/moq-wasm/blob/master/sdk/python/README.md) and [Rust](https://github.com/nttcom/moq-wasm/blob/master/sdk/rust/README.md) SDKs.

## Entry points

| Import path          | Use for                                                                                                                                |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `moq-wasm`           | `MoqSession`, raw tracks (`publishTrack`, `subscribe`, `fetch`), namespace watches                                                     |
| `moq-wasm/media`\*   | `createBroadcast`, `LivePlayer`, `RemoteBroadcast`, `PhoneCall`, `watchBroadcasts`, `subscribeTranscript`, `botNamespace`, `botTypeOf` |
| `moq-wasm/element`   | `<moq-player>` custom HTML element                                                                                                     |
| `moq-wasm/recording` | `openRecording` for replaying recorded MoQ archives                                                                                    |

\*`moq-wasm/media` is unstable and may change in any minor release.

## Install

In an empty directory, install Vite and the latest SDK release:

```shell
npm install -D vite
npm install https://github.com/nttcom/moq-wasm/releases/download/js-sdk-v0.9.0/moq-wasm-0.9.0.tgz
```

Exclude `moq-wasm` from Vite pre-bundling in `vite.config.mjs` so Vite can load its `.wasm` binary and workers:

```js
export default { optimizeDeps: { exclude: ['moq-wasm'] } }
```

## Quickstart: Publish your camera

Create `index.html`:

```html
<button id="publish">Publish camera</button>
<video autoplay muted playsinline></video>
<output></output>
<script type="module" src="/main.js"></script>
```

Create `main.js`. This connects to the public demo relay on a random `anon` namespace (anyone with that namespace can watch while the tab stays open):

```js
import { MoqSession } from 'moq-wasm'
import { createBroadcast } from 'moq-wasm/media'

const button = document.querySelector('#publish')
button.onclick = async () => {
  button.disabled = true
  const session = await MoqSession.connect('https://relay-1.moqt.research.skyway.io:443')
  const namespace = ['anon', crypto.randomUUID()]
  const broadcast = await createBroadcast(session, namespace)
  const camera = await broadcast.addSource('camera', { video: true })
  document.querySelector('video').srcObject = camera.stream
  document.querySelector('output').textContent = namespace.join('/')
}
```

Try it:

1. Run `npx vite` and open the local URL in Chrome.
2. Click **Publish camera** and allow camera access — your preview and namespace appear.
3. Open [Live Viewer](https://nttcom.github.io/moq-wasm/examples/live-viewer/?access=public), select **relay-1**, paste the namespace, and click **Watch**. Close the publisher tab to stop.

## Watch a broadcast

### Option A: `LivePlayer`

Create `watch.html` and `watch.js` next to the publisher:

```html
<input placeholder="namespace" />
<button id="watch">Watch</button>
<div id="player"></div>
<script type="module" src="/watch.js"></script>
```

```js
import { MoqSession } from 'moq-wasm'
import { LivePlayer } from 'moq-wasm/media'
import 'moq-wasm/media/player.css'

const player = new LivePlayer({ container: document.querySelector('#player') })

document.querySelector('#watch').onclick = async () => {
  const session = await MoqSession.connect('https://relay-1.moqt.research.skyway.io:443')
  await player.start(session, document.querySelector('input').value.split('/'))
}
```

Open `/watch.html` in another tab, paste the publisher's namespace, and click **Watch** (calling `player.start()` inside a click handler lets the browser play audio). Also import `moq-wasm/media/controls.css` if you attach `PlayerControls`.

### Option B: `<moq-player>` element

`<moq-player>` embeds a player without constructing `LivePlayer` manually:

```html
<input placeholder="namespace" />
<button id="watch">Watch</button>
<moq-player controls></moq-player>
<script type="module" src="/watch.js"></script>
```

```js
import 'moq-wasm/element'
import 'moq-wasm/media/player.css'
import 'moq-wasm/media/controls.css'

const player = document.querySelector('moq-player')
document.querySelector('#watch').onclick = () => {
  player.setAttribute('namespace', document.querySelector('input').value)
  player.setAttribute('src', 'https://relay-1.moqt.research.skyway.io:443')
}
```

Playback starts while the element is in the document with `namespace` and `src` (or `session`), restarts when `src`, `session`, `namespace`, `token`, or `tracks` changes, and stops on removal (closing any session opened via `src`).

| Attribute / property | Description                                                                                |
| -------------------- | ------------------------------------------------------------------------------------------ |
| `src`                | Relay URL                                                                                  |
| `namespace`          | `/`-separated track namespace                                                              |
| `token`              | Auth JWT string (attribute), or `() => Promise<string>` returning a fresh token (property) |
| `controls`           | Show seek bar and quality, audio, and buffer controls                                      |
| `packaging`          | `loc` (default) or `cmaf`; changing while playing switches without restarting              |
| `session` (property) | Existing `MoqSession` to reuse instead of `src` (never closed by the element)              |
| `tracks` (property)  | Explicit tracks to play instead of catalog discovery (same shape as `LivePlayer.start`)    |
| `player` (read-only) | Active `LivePlayer` while playing (for `setVolume`, `stats`, etc.)                         |

Events: `statechange`, `liveframe` (mirroring `LivePlayerCallbacks`), and `error` (`ErrorEvent`) on connect/start failure. See the [Embed example](https://github.com/nttcom/moq-wasm/tree/master/examples/browser/examples/embed).

## Room bots and captions

Bots launched in a room publish at `[...room, 'bots', botType]`. Pass `onBotChange` to `watchBroadcasts` to track when bots enter or leave (including bots already in the room), and call `subscribeTranscript` while the `transcriber` bot is present:

```js
import { botTypeOf, subscribeTranscript, watchBroadcasts } from 'moq-wasm/media'

const room = ['app-id', 'room-1']
let transcript // the latest subscribeTranscript() promise, if the transcriber is in the room

const watch = await watchBroadcasts(session, room, {
  // Exclude bot namespaces if you only play human participants.
  filter: (namespace) => botTypeOf(room, namespace) === undefined,
  onBotChange: (botType, present) => {
    setBotBadge(botType, present)
    if (botType !== 'transcriber') return
    // Close the subscription of the transcriber's previous stay, even one still being set up.
    transcript?.then((subscription) => subscription.close()).catch(() => {})
    transcript = present ? subscribeTranscript(session, room, renderTranscript) : undefined
    transcript?.catch(console.error)
  },
  onAdd: (broadcast) => addParticipant(broadcast),
  onRemove: (broadcast) => removeParticipant(broadcast)
})
```

- `onBotChange` is independent of `filter` and playback: a bot namespace another session published is still passed to `filter` and played unless excluded, while a bot this session published is only reported. Bots are reported on the same watch because a relay rejects a second `SUBSCRIBE_NAMESPACE` with an overlapping prefix (`NAMESPACE_PREFIX_OVERLAP`). `onBotChange` is not called on `watch.close()`.
- `subscribeTranscript` follows the `transcript` track of `botNamespace(room, 'transcriber')` with a joining FETCH and live subscription, skipping malformed objects and records. `onRecords` receives every record in the order the transcriber published them each time they change; history from the joining FETCH has `live === false`, and `onRecords` is not called after `close()`. It rejects with `RequestError` when the transcriber is absent, and its returned `TranscriptSubscription` exposes `done` (resolving on `PUBLISH_DONE`, or `undefined` if the session ends first) and `close()`.
- Pages reading `session.watchNamespaces(room)` directly can use `botTypeOf(room, namespace)` to recognize bot namespaces and `botNamespace(room, botType)` to build them.

## Authentication

Pass a static JWT or an async provider to `MoqSession.connect` (called before every `CLIENT_SETUP`, including reconnects and `GOAWAY` moves):

```js
const session = await MoqSession.connect('https://relay.example.com:443', {
  token: async () => fetchTokenFromYourServer()
})
```

## Reference & examples

- **API docs**: [MoqSession](https://nttcom.github.io/moq-wasm/docs/js/classes/core.MoqSession.html), [LivePlayer](https://nttcom.github.io/moq-wasm/docs/js/classes/media.LivePlayer.html), [LivePlayerOptions](https://nttcom.github.io/moq-wasm/docs/js/types/media.LivePlayerOptions.html), [recording](https://nttcom.github.io/moq-wasm/docs/js/modules/recording.html)
- **Multi-party meeting**: [Meeting example](https://github.com/nttcom/moq-wasm/tree/master/examples/browser/examples/meeting) (`watchBroadcasts()`)
- **Changelog**: [CHANGELOG.md](https://github.com/nttcom/moq-wasm/blob/master/sdk/js/js/CHANGELOG.md)

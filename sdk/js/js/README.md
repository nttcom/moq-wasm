# moq-wasm for JavaScript

Publish a camera from a browser, then watch it through a MoQT relay.

## Install

In an empty directory, install Vite and the SDK from its GitHub release:

```shell
npm install -D vite
npm install https://github.com/nttcom/moq-wasm/releases/download/js-sdk-v0.6.0/moq-wasm-0.6.0.tgz
```

In `vite.config.mjs`, exclude the SDK from pre-bundling so Vite can load its
WebAssembly file and workers:

```js
export default { optimizeDeps: { exclude: ['moq-wasm'] } }
```

## Publish a camera

Create `index.html`:

```html
<button id="publish">Publish camera</button>
<video autoplay muted playsinline></video>
<output></output>
<script type="module" src="/main.js"></script>
```

Create `main.js`. This example publishes video on the public demo relay.
Anyone with the namespace can watch while the page is open.

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

Run `npx vite`, open the local URL in Chrome, click **Publish camera** and allow
camera access. Your preview and namespace appear. To see the stream from a
subscriber, open [Live Viewer](https://nttcom.github.io/moq-wasm/examples/live-viewer/?access=public),
select **relay-1**, enter that namespace and click **Watch**. Close the publisher tab to stop.

## Watch with a tag

`<moq-player>` plays a broadcast without building a `LivePlayer`. Next to the
publisher, create `watch.html`:

```html
<input placeholder="namespace" />
<button id="watch">Watch</button>
<moq-player controls></moq-player>
<script type="module" src="/watch.js"></script>
```

Create `watch.js`. Importing `moq-wasm/element` registers the tag, and the two
style sheets lay out the picture and the controls:

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

Open `/watch.html` in another tab, enter the namespace the publisher shows and
click **Watch**. `src` is set from a click so the browser allows audio playback.

The element plays while it is in the page with `namespace` and either `src` or
`session`, starts again when one of them, the token or `tracks` changes, and
stops when it is removed, closing the session it opened from `src`.

| Attribute or property |                                                                                                               |
| --------------------- | ------------------------------------------------------------------------------------------------------------- |
| `src`                 | The relay URL                                                                                                 |
| `namespace`           | The track namespace, `/`-separated                                                                            |
| `token`               | The token, as an attribute; the property also takes a function that returns a fresh token before each connect |
| `controls`            | Shows the seek bar and the quality, audio and buffer controls                                                 |
| `packaging`           | `cmaf` plays the CMAF tracks; `loc` by default. Changing it while playing does not restart                    |
| `session` (property)  | A session the page opened, played on instead of connecting to `src`; the element never closes it              |
| `tracks` (property)   | The tracks to play instead of those the catalog lists, as in `LivePlayer.start`                               |
| `player` (read-only)  | The `LivePlayer` while it plays, for `setVolume`, `stats` and the rest                                        |

It fires `statechange` and `liveframe` as `LivePlayerCallbacks.onStateChange`
and `onLiveFrame` are called, and an `error` `ErrorEvent` when it fails to
connect or start. See the
[Embed example](https://github.com/nttcom/moq-wasm/tree/master/examples/browser/examples/embed).

## Build on the example

- **Play a broadcast.** Call `LivePlayer.start()` from a click so the browser
  allows audio playback. See the [player methods](https://nttcom.github.io/moq-wasm/docs/js/classes/media.LivePlayer.html)
  and [player options](https://nttcom.github.io/moq-wasm/docs/js/types/media.LivePlayerOptions.html).
- **Build a meeting.** The [Meeting example](https://github.com/nttcom/moq-wasm/tree/master/examples/browser/examples/meeting)
  uses `watchBroadcasts()` to discover participants and display their sources.
- **Send data or replay a recording.** See the [session API](https://nttcom.github.io/moq-wasm/docs/js/classes/core.MoqSession.html)
  and [recording API](https://nttcom.github.io/moq-wasm/docs/js/modules/recording.html).

The `moq-wasm/media` API may change in any minor release.

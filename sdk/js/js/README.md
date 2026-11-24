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

## Build on the example

- **Play a broadcast.** Call `LivePlayer.start()` from a click so the browser
  allows audio playback. See the [player methods](https://nttcom.github.io/moq-wasm/docs/js/classes/media.LivePlayer.html)
  and [player options](https://nttcom.github.io/moq-wasm/docs/js/types/media.LivePlayerOptions.html).
- **Embed a player with a tag.** Import `moq-wasm/element` and write
  `<moq-player src="…" namespace="anon/…" controls></moq-player>`. It connects
  once it has `src` and `namespace`, and closes the session when removed. Set
  `token` as an attribute, or as a property holding a function that returns a
  fresh one. Set `src` from a click for audio playback. A page that opens its
  own session sets the `session` property instead of `src`. See the
  [Embed example](https://github.com/nttcom/moq-wasm/tree/master/examples/browser/examples/embed).
- **Build a meeting.** The [Meeting example](https://github.com/nttcom/moq-wasm/tree/master/examples/browser/examples/meeting)
  uses `watchBroadcasts()` to discover participants and display their sources.
- **Send data or replay a recording.** See the [session API](https://nttcom.github.io/moq-wasm/docs/js/classes/core.MoqSession.html)
  and [recording API](https://nttcom.github.io/moq-wasm/docs/js/modules/recording.html).

The `moq-wasm/media` API may change in any minor release.

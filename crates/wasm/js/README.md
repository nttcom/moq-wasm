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
```

`moq-wasm/media` stays below 1.0 whatever the package version: any minor
version may change it. Import
`moq-wasm/media/controls.css` as well with `PlayerControls`.

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

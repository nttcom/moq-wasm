# wasm

WebAssembly bindings that expose `moqt` to the browser over WebTransport.
`Session` (`src/session.rs`) runs `moqt::Endpoint::<BROWSER>` (the session
stack of `crates/moqt` on the browser's WebTransport API) and hands its
requests and objects to JavaScript as handles and async reads.
The `moq-wasm` npm package in `js/` wraps the generated glue (`js/pkg`) in
`MoqSession`, `publish()` and `LivePlayer`; `examples/browser` depends on it
as an npm workspace.

```shell
npm --prefix crates/wasm/js run wasm         # wasm-pack build --target web
```

`make browser` builds it before starting Vite, so this is only needed when
you want the package without the dev server.

Exports `Session` (SUBSCRIBE, FETCH, TRACK_STATUS, SUBSCRIBE_NAMESPACE,
PUBLISH_NAMESPACE and the tracks it publishes), `MediaBroadcast`, the MSF
catalog parser from `crates/msf`,
LOC helpers from `crates/mediapack` and the progressive MP4 index the Live
Viewer's MP4 publisher uses.

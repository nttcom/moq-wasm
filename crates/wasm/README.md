# wasm

WebAssembly bindings that expose `moqt` to the browser over WebTransport.
`MOQTClient` runs `moqt::Endpoint::<BROWSER>` (the session stack of
`crates/moqt` on the browser's WebTransport API) and maps its requests,
responses and objects onto the JavaScript callback API in `src/client.rs`.
`examples/browser` consumes the generated package from `examples/browser/pkg`.

```shell
npm --prefix examples/browser run wasm       # wasm-pack build --target web
```

`make browser` builds it before starting Vite, so this is only needed when
you want the package without the dev server.

Exports `MOQTClient` (sessions, PUBLISH/SUBSCRIBE/FETCH and the incoming
FETCH handler), the MSF catalog types from `crates/msf`,
LOC helpers from `crates/mediapack` and the progressive MP4 index the Live
Viewer's MP4 publisher uses.

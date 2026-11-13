# wasm

WebAssembly bindings that expose `moqt` to the browser over WebTransport.
`Session` (`src/session.rs`) runs `moqt::Endpoint::<BROWSER>` and hands its
requests and objects to JavaScript. The crate also exports `MediaBroadcast`,
the MSF catalog parser and LOC helpers.

The `moq-wasm` npm package in [`js/`](js/README.md) wraps the generated glue
in `MoqSession`, `createBroadcast()` and `LivePlayer`; `examples/browser`
depends on it as an npm workspace.

```shell
npm --prefix crates/wasm/js run wasm         # wasm-pack build --target web
```

`make browser` runs this before starting Vite.

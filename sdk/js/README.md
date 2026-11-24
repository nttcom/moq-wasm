# wasm

> Looking for the JavaScript/TypeScript browser SDK? See [`sdk/js/js/README.md`](js/README.md).

Internal Rust/WebAssembly bindings exposing `moqt::Endpoint::<BROWSER>`, `MediaBroadcast`, MSF catalog parsing, and LOC helpers to the `moq-wasm` npm package in [`js/`](js/README.md).

```shell
npm --prefix sdk/js/js run wasm   # wasm-pack build --target web (also run by `make browser`)
```

# [S] Export the types moq-wasm's public API refers to

Next: run `npm --prefix examples/browser run docs:js` and go through its 12 "not included in the documentation" warnings.

## Goal

`npm --prefix examples/browser run docs:js` reports no "is referenced by … but not included in the documentation" warning, so every type a public signature names has its own page in the API reference.

## Plan

Export the user-facing ones (`SeekAxis`, `MediaKind`, `StatusState`, `CatalogTrackRole`, `LogLevel`, `ReviewFetchWindow`, `Fields`) from the `core` or `media` entry point.
The `Wasm*` types appear only as `inner` of public constructors (`MoqSession`, `MoqFetch`, …): mark those constructors `@internal` instead of exporting the wasm-bindgen types.

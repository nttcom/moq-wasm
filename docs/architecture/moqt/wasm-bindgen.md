# `wasm-bindgen`, `wasm-bindgen-futures`, and `js-sys` for the Browser Executor (`moqt`)

**Status:** Accepted (2026-10-05)

## What

Add `wasm-bindgen`, `wasm-bindgen-futures`, and `js-sys` as `wasm32`-only dependencies of `moqt` to implement `modules/executor/browser.rs`:

- `spawn_local`: runs session background tasks on the JS event loop.
- `timeout`: wraps global `setTimeout` in a `Promise`.
- `yield_now`: awaits an already-resolved `Promise`.

## Context

`modules/moqt` runs receive loops as background tasks and bounds every control-message wait with a timeout. Native builds use Tokio; browsers have no Tokio runtime, so tasks run on the JavaScript event loop and timers come from `setTimeout`.

## Alternatives

- **`gloo-timers`:** Wraps `setTimeout`, adding an extra crate for a single function.
- **`tokio` (`rt` feature) on `wasm32`:** `tokio::time` panics on `wasm32-unknown-unknown`, and the runtime must still be polled from a JS task—which `wasm-bindgen-futures` already handles.
- **`web-sys` `Window::set_timeout_*`:** Fails inside Web Workers (no `Window`); `js_sys::global()` works in both windows and workers.

## Decision

Use the three `wasm-bindgen` crates directly. `sdk/js` already depends on them at the same versions, adding no new crate to the dependency tree and keeping one copy of the JS glue.

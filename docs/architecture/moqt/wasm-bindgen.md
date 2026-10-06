# wasm-bindgen, wasm-bindgen-futures and js-sys for the browser executor

## Status

Accepted

## Date

2026-10-05

## What

Add `wasm-bindgen`, `wasm-bindgen-futures` and `js-sys` as
`wasm32`-only dependencies of the `moqt` crate. They back
`modules/executor/browser.rs`: `spawn_local` runs the session's background
tasks, a `Promise` wrapped around the global `setTimeout` implements
`timeout`, and an already-resolved `Promise` implements `yield_now`.

## Context

`modules/moqt` runs its receive loops as background tasks and bounds every
control-message wait with a timeout. Natively both come from tokio. The
browser has no tokio runtime: tasks run on the JavaScript event loop and
timers come from `setTimeout`, so the executor needs a way to reach both
from Rust.

## Alternatives

- `gloo-timers` for the timer. Wraps the same `setTimeout` call; one more
  crate for one function.
- `tokio` with the `rt` feature on wasm32. tokio's current-thread runtime
  compiles for `wasm32-unknown-unknown`, but `time` panics there and the
  runtime still has to be polled from a JavaScript task, which is what
  `wasm-bindgen-futures` already does.
- `web-sys` `Window::set_timeout_*`. Fails inside a Worker, where there is no
  `Window`; `js_sys::global()` works in both.

## Decision

Use the three `wasm-bindgen` crates directly. `crates/wasm` already depends on
them at the same versions, so no new crate enters the dependency tree, and the
browser bindings keep a single copy of the JavaScript glue.

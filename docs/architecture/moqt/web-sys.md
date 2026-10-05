# web-sys for the browser transport

## Status

Accepted

## Date

2026-10-05

## What

Add `web-sys` as a `wasm32`-only dependency of the `moqt` crate, with the
`WebTransport*`, `ReadableStream*` and `WritableStream*` features.
`modules/transport/browser` implements the transport traits on top of the
browser's `WebTransport` object, so `Endpoint::<BROWSER>` runs the same
session stack as the native markers.

## Context

The browser offers exactly one QUIC transport, `WebTransport`, and only
through its JavaScript API. The session stack is written against
`TransportConnection`, `TransportSendStream` and `TransportReceiveStream`; a
browser implementation of those three needs typed access to `WebTransport`,
its bidirectional and unidirectional streams, datagrams, close info and
`WebTransportError`.

## Alternatives

- `web-transport-wasm` (same author as `web-transport-quinn`). Wraps the same
  `web-sys` calls behind a `Session` / `SendStream` / `RecvStream` API. The
  transport traits here already are that abstraction, so the extra layer
  would only add a second error type to map.
- `js-sys::Reflect` for everything. Avoids the feature list but turns every
  method call into a string lookup and loses the generated types.

## Decision

Use `web-sys` directly, limited to the listed features. `crates/wasm` already
depends on it at the same version, so no new crate enters the dependency
tree. The `WebTransport` types are still behind `--cfg web_sys_unstable_apis`
in `web-sys` 0.3; the root `.cargo/config.toml` sets that cfg for every build
in the workspace, replacing the copy that lived in `crates/wasm/.cargo`.
`streamErrorCode` and `sendOrder` are read and written through
`js_sys::Reflect` because `web-sys` types the former as `u8` and does not
expose the latter.

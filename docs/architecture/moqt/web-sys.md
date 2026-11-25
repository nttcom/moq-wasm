# `web-sys` for the Browser Transport (`moqt`)

**Status:** Accepted (2026-10-05)

## What

Add `web-sys` as a `wasm32`-only dependency of `moqt` with features:

- **WebTransport & streams:** `WebTransport*`, `ReadableStream*`, `WritableStream*`, `Url`
- **QMux over WebSocket & clock:** `WebSocket`, `BinaryType`, `CloseEvent`, `Event`, `MessageEvent`, `Performance`

`modules/transport/browser` implements the transport traits over browser `WebTransport` so `Endpoint::<BROWSER>` shares the exact session stack as native markers.

## Context

Browsers expose QUIC exclusively via the JavaScript `WebTransport` API. Implementing `TransportConnection`, `TransportSendStream`, and `TransportReceiveStream` requires typed bindings for `WebTransport`, its bidirectional/unidirectional streams, datagrams, close info, and `WebTransportError`.

## Alternatives

- **`web-transport-wasm`:** Wraps `web-sys` behind `Session`/`SendStream`/`RecvStream`, duplicating `moqt`'s existing transport traits and adding an extra error mapping layer.
- **`js_sys::Reflect` for everything:** Avoids feature flags but converts every call into an untyped string lookup.

## Decision

Use `web-sys` directly, scoped to the listed features. Because `web-sys` 0.3 gates `WebTransport` behind `--cfg web_sys_unstable_apis`, the workspace root `.cargo/config.toml` sets it globally (replacing the old `sdk/js/.cargo` copy).

`streamErrorCode` and `sendOrder` still use `js_sys::Reflect`, because `web-sys` types `streamErrorCode` as `u8` and omits `sendOrder`.

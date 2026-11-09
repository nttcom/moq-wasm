# tokio-tungstenite for the Gemini Live API connection

## Status

Proposed

## Date

2026-10-06

## What

Add `tokio-tungstenite` (with the `connect` and `rustls-tls-native-roots`
features) to the `live-translate` crate. It opens the WebSocket to the Gemini Live
API on the Agent Platform (`wss://aiplatform.googleapis.com/ws/...BidiGenerateContent`) that
carries one translation session per target language.

## Context

Gemini Live Translate (`gemini-3.5-live-translate-preview`) is only reachable
through the Live API, a bidirectional JSON-over-WebSocket protocol: the client
sends a `setup` message, streams 16 kHz PCM in `realtimeInput` messages and
receives translated 24 kHz PCM in `serverContent` messages on the same socket.
The workspace had no WebSocket client. `reqwest` (already present) only speaks
request/response HTTP. There is no official Google SDK for Rust.

## Alternatives

- Run the translation in a separate process with an official SDK (Python or
  Node) and bridge PCM to the Rust translator. This adds a second runtime and an
  inter-process audio hop to a path whose only job is to forward PCM.
- `tungstenite` without the Tokio binding, driven from a blocking thread. The
  translator is Tokio-based; a blocking socket per language would need its own
  thread and channel plumbing that `tokio-tungstenite` already provides.
- Other async WebSocket crates (`async-tungstenite`, `fastwebsockets`). Both
  work, but `tokio-tungstenite` is the most widely used Tokio binding and needs
  no extra executor glue.

## Decision

Use `tokio-tungstenite` with rustls. The TLS configuration is built explicitly
with rustls' `ring` provider and the platform's native roots, the same choices
`moqt` makes for QUIC, so the process ends up with one TLS stack and one
provider regardless of which connection is opened first.

## Consequences

### Positive
- The Live API protocol is implemented in about a hundred lines over typed
  `serde` messages, in the same process and runtime as the MoQT session.
- No native TLS library or additional language runtime in the container image.

### Negative
- Session resumption, `goAway` handling and reconnects are ours to maintain;
  the official SDKs would provide them.

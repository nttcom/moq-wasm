# `tokio-tungstenite` for Gemini Live API WebSockets (`live-translate`)

**Status:** Accepted (2026-10-06)

## What

Add `tokio-tungstenite` (features: `connect`, `rustls-tls-native-roots`) to `live-translate` to open WebSocket connections to the Gemini Live API (`wss://aiplatform.googleapis.com/ws/...BidiGenerateContent`), one session per target language.

## Context

Gemini Live Translate (`gemini-3.5-live-translate-preview`) is exposed only over the bidirectional JSON-over-WebSocket Live API (`setup`, `realtimeInput` with 16 kHz PCM, `serverContent` with 24 kHz translated PCM). The workspace had no WebSocket client—`reqwest` is request/response HTTP only—and there is no official Google Rust SDK.

## Alternatives

- **Separate Python/Node process using an official SDK + IPC audio bridge:** Adds a second language runtime and an extra inter-process audio hop just to forward PCM.
- **Blocking `tungstenite` on a dedicated thread per language:** Duplicates thread and channel plumbing inside a Tokio service.
- **`async-tungstenite` / `fastwebsockets`:** Viable, but `tokio-tungstenite` is the standard Tokio binding and needs no extra executor glue.

## Decision

Use `tokio-tungstenite` with `rustls`, explicitly configured with the `aws-lc-rs` provider and platform native roots—matching `moqt`'s QUIC TLS stack so the process uses one TLS implementation and provider regardless of connection order.

## Consequences

- **Positive:** The Live API client fits in ~100 lines of typed `serde` messages inside the same process and Tokio runtime as MoQT, with no native TLS library or second language runtime in the container.
- **Negative:** Session resumption, `goAway` handling, and reconnects are maintained in-house instead of by an official SDK.

# recording-player

Multi-participant replay player for finished MoQ archives served by [`services/recorder`](../../../../services/recorder/README.md) (`recorder serve`), using `openRecording` (`moq-wasm/recording`) and `PlaybackGroup` (`moq-wasm/media`).

## Run

```shell
make relay
make recorder-serve   # http://127.0.0.1:8096
make browser
make chrome
```

Open `http://localhost:5173/moq-wasm/examples/recording-player/`, enter the **Recorder URL**, **Recording ID**, **Relay URL**, and optional **Access token** (pre-filled from `?recorder=`, `?id=`, and the current access mode), then click **再生する**.

## Features

- **Synchronized multi-tile playback**: Discovers all replayed participant camera and screen-share namespaces under the recording prefix and keeps every tile aligned via `PlaybackGroup`.
- **Transport controls**: Play/pause and timeline scrubbing across the full recording span.
- **Event sidebar**: Fetches MSF event timeline tracks (`chat` and `transcript`) and reveals messages/captions synchronized with the playhead.

## E2E

```shell
LIVE_VIEWER_E2E_TRANSCODE=0 node tests/browser-e2e/run-live-viewer-e2e.mjs tests/recorder-archive-e2e.spec.ts
```

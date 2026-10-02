# moq-wasm

## What is moq-wasm?

Real-time communication of any data for web browsers, native apps, IP
cameras, USB cameras, robots, drones and AI agents. One publish/subscribe API
with a simple architecture, high-performance QUIC transport, priority control
and caching, so you no longer have to pick between WebSocket, MQTT, HLS, DASH
and WebRTC for each use case.

Implements Media over QUIC Transport
[draft-ietf-moq-transport-14](https://datatracker.ietf.org/doc/draft-ietf-moq-transport/14/)
in Rust (drafts in [`spec/`](spec/)).

## Use cases and demos

Try the demos hosted at **<https://nttcom.github.io/moq-wasm/>**. They
connect to the cloud relay by default, so nothing needs to be installed.

| Use case | Demo |
| --- | --- |
| Video meetings | [Meeting](https://nttcom.github.io/moq-wasm/examples/meeting/) |
| Large-scale live streaming | [Live Viewer](https://nttcom.github.io/moq-wasm/examples/live-viewer/): live playback with rewind and rendition switching |
| Video on demand | [Live Viewer](https://nttcom.github.io/moq-wasm/examples/live-viewer/): publish an MP4 from the browser |
| Remote monitoring and control of IP cameras | [ONVIF](https://nttcom.github.io/moq-wasm/examples/onvif/): PTZ control and monitoring |
| Remote monitoring from USB cameras | [Remote Monitoring](https://nttcom.github.io/moq-wasm/examples/remote-monitoring/) |
| Real-time chat moderation by an AI agent | [MoQ Chat Moderation](https://nttcom.github.io/moq-wasm/examples/moq-chat-moderation/) |
| Real-time video analysis by an AI agent | [MoQ Camera Detection](https://nttcom.github.io/moq-wasm/examples/moq-camera-detection/): answers questions about a camera feed |
| Remote monitoring, data collection and control of drones, robots and autonomous vehicles | |

Low-level API pages: [Message](https://nttcom.github.io/moq-wasm/examples/message/),
[Media](https://nttcom.github.io/moq-wasm/examples/media/),
[Media CMAF](https://nttcom.github.io/moq-wasm/examples/media-cmaf/),
[WebCodecs](https://nttcom.github.io/moq-wasm/examples/webcodecs/).

Publishers outside the browser: [`bridges/live-ingest`](bridges/live-ingest/README.md) (RTMP/SRT),
[`bindings/gstreamer`](bindings/gstreamer/README.md) (`moqtsink`),
[`examples/rust/moq-cli`](examples/rust/moq-cli/README.md) and the
[pipecat bots](examples/python). Details of every browser demo are in
[`examples/browser/README.md`](examples/browser/README.md).

## Getting Started

```shell
nix develop
npm --prefix examples/browser ci
```

Run a relay, publish a test stream and watch it, in four terminals:

```shell
make relay          # 1. relay and a local VTS on https://127.0.0.1:4433
make live-ingest    # 2. RTMP/SRT ingest publishing to the relay
make ffmpeg-srt     # 3. test pattern over SRT into the ingest
make browser        # 4. wasm build and the example hub on http://localhost:5173
```

Then `make chrome` (`make chrome:linux` on Linux) opens Chrome trusting the
relay's self-signed certificate. Open Live Viewer from the hub and watch
`anon/live/test`.

### Make targets

| Target | Does |
| --- | --- |
| `relay` | Relay with a local VTS; the certificate is generated into `relay/keys/` on first run |
| `browser`, `chrome`, `chrome:linux` | Vite dev server with the wasm build; Chrome pinned to the relay certificate |
| `live-ingest`, `live-ingest-transcode`, `live-ingest-stats` | RTMP/SRT bridge; with lower renditions; with QUIC statistics |
| `gst-plugin`, `gst-srt-publish` | Build `moqtsink`; SRT into MoQT through `gst-launch-1.0` |
| `onvif` | ONVIF camera bridge, configured in `.env` |
| `ffmpeg-rtmp`, `ffmpeg-srt`, `ffmpeg-srt-bbb-local`, `ffmpeg-srt-bbb-remote` | Test sources: pattern over RTMP or SRT, Big Buck Bunny to a local or the cloud ingest |
| `test`, `lint`, `format` | `cargo test`; clippy and `tsc`; rustfmt and prettier |
| `browser-e2e-media`, `browser-e2e-live-viewer`, `browser-e2e-meeting`, `browser-e2e-meeting-headed` | Playwright E2E; relay and dev server are started for you |
| `relay-certs` | Generate the relay certificate without starting the relay |

### Authentication

Relays always authenticate. A session without a token is limited to
`anon/**`, which every example uses by default. Tokens are HS256 JWTs verified
by the VTS; mint one with `services/vts/bin/mint.mjs`. See
[`services/vts/README.md`](services/vts/README.md).

### Two relays with Docker Compose

```shell
docker compose up -d
```

Starts `relay-a` (`https://127.0.0.1:4433`) and `relay-b` (`:4434`) sharing a
VTS and a Redis route registry, the topology the cascading, auth and meeting
E2E use. The `make` targets resolve the Docker Desktop bridge host for native
clients on macOS.

### Tests

```shell
make test                        # Rust unit tests
make browser-e2e-media           # browser publish/subscribe
./scripts/auth-e2e.sh            # relay scenarios, see tests/README.md
npm --prefix services/vts test   # VTS
```

## Architecture

```mermaid
flowchart LR
    subgraph Publishers
        B1[Browser<br/>bindings/wasm]
        LI[live-ingest<br/>RTMP / SRT]
        GS[moqtsink<br/>GStreamer]
        ON[onvif bridge]
        CLI[moq-cli]
    end
    subgraph Relays
        RA[relay-a] <--> RB[relay-b]
        VTS[VTS<br/>JWT verification]
        RA -.-> VTS
        RB -.-> VTS
    end
    subgraph Subscribers
        B2[Browser<br/>Live Viewer, Meeting, ...]
        BOT[AI agents<br/>pipecat bots]
        CLI2[moq-cli]
    end
    Publishers -- PUBLISH / SUBSCRIBE / FETCH<br/>QUIC or WebTransport --> RA
    RB --> Subscribers
```

Every client speaks MoQT to a relay over QUIC or WebTransport. Relays cache
objects for FETCH, forward subscriptions to each other and authenticate
sessions through the VTS. The `moqt` crate is the protocol library every other
component builds on.

| Path | Component |
| --- | --- |
| [`moqt/`](moqt/README.md) | Protocol library: wire format, sessions, publisher and subscriber API |
| [`relay/`](relay/README.md) | Relay: QUIC and WebTransport on one port, cache, FETCH, cascading, JWT auth |
| [`services/vts/`](services/vts/README.md) | Verify Token Service: checks client JWTs for the relay |
| [`bindings/`](bindings/README.md) | `wasm` for browsers, `gstreamer` (`moqtsink`) for GStreamer pipelines |
| [`bridges/`](bridges/README.md) | `live-ingest` (RTMP/SRT) and `onvif` (cameras) into MoQT |
| [`shared/`](shared/README.md) | `mediapack` containers, `media-streaming-format` catalog, `media-publisher`, `transcode` |
| [`examples/`](examples/browser/README.md) | Browser examples, `moq-cli`, pipecat bots |
| [`tests/`](tests/README.md) | Relay E2E scenarios: auth, FETCH, cache eviction, cascading, dedup |
| [`architecture_decision_record/`](architecture_decision_record) | Architecture and dependency decisions per crate |

Architecture documents:
[`moqt`](architecture_decision_record/moqt/architecture.md),
[`relay`](architecture_decision_record/relay/architecture.md),
[`media-publisher`](architecture_decision_record/media-publisher/architecture.md),
[Live Player](architecture_decision_record/browser-examples/live-player.md).
Contributor rules: [`AGENTS.md`](AGENTS.md).

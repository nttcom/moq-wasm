# MoQT

Media over QUIC Transport in Rust: the protocol library, a relay, and the
publishers and players around them. Implements draft-ietf-moq-transport-14
(drafts in [`spec/`](spec/)).

## What is here

| Path | Component |
| --- | --- |
| [`moqt/`](moqt/README.md) | Protocol library: wire format, sessions, publisher and subscriber API |
| [`relay/`](relay/README.md) | Relay: QUIC and WebTransport on one port, cache, FETCH, cascading, JWT auth |
| [`services/vts/`](services/vts/README.md) | Verify Token Service: checks client JWTs for the relay |
| [`bindings/`](bindings/README.md) | `wasm` for browsers, `gstreamer` (`moqtsink`) for GStreamer pipelines |
| [`bridges/`](bridges/README.md) | `live-ingest` (RTMP/SRT) and `onvif` (cameras) into MoQT |
| [`shared/`](shared/README.md) | `mediapack` containers, `media-streaming-format` catalog, `media-publisher`, `transcode` |
| [`examples/browser/`](examples/browser/README.md) | Browser demos: Live Viewer, Meeting, ONVIF, AI bots and the low-level API pages |
| [`examples/rust/moq-cli/`](examples/rust/moq-cli/README.md) | CLI that pipes H.264 in and out of a relay |
| [`examples/python/`](examples/python) | pipecat bots: chat moderation and camera detection |
| [`tests/`](tests/README.md) | Relay E2E scenarios: auth, FETCH, cache eviction, cascading, dedup |
| [`architecture_decision_record/`](architecture_decision_record) | Architecture and dependency decisions per crate |

## Quick start

```shell
nix develop
npm --prefix examples/browser ci
```

Four terminals:

```shell
make relay          # 1. relay and a local VTS on https://127.0.0.1:4433
make live-ingest    # 2. RTMP/SRT ingest publishing to the relay
make ffmpeg-srt     # 3. test pattern over SRT into the ingest
make browser        # 4. wasm build and the demo hub on http://localhost:5173
```

Then `make chrome` (`make chrome:linux` on Linux) opens Chrome trusting the
relay's self-signed certificate. Open Live Viewer from the hub and watch
`anon/live/test`.

## Make targets

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

## Authentication

Relays always authenticate. A session without a token is limited to
`anon/**`, which every demo uses by default. Tokens are HS256 JWTs verified by
the VTS; mint one with `services/vts/bin/mint.mjs`. See
[`services/vts/README.md`](services/vts/README.md).

## Two relays with Docker Compose

```shell
docker compose up -d
```

Starts `relay-a` (`https://127.0.0.1:4433`) and `relay-b` (`:4434`) sharing a
VTS and a Redis route registry, the topology the cascading, auth and meeting
E2E use. Native clients on macOS reach them through the Docker Desktop bridge
host; the `make` targets resolve that for you.

## Tests

```shell
make test                        # Rust unit tests
make browser-e2e-media           # browser publish/subscribe
./scripts/auth-e2e.sh            # relay scenarios, see tests/README.md
npm --prefix services/vts test   # VTS
```

## Documents

- Architecture: [`moqt`](architecture_decision_record/moqt/architecture.md), [`relay`](architecture_decision_record/relay/architecture.md), [`media-publisher`](architecture_decision_record/media-publisher/architecture.md), [Live Player](architecture_decision_record/browser-examples/live-player.md)
- Contributor rules: [`AGENTS.md`](AGENTS.md)

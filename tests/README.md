# tests

Run from the repository root:

```shell
make test               # Rust unit tests
make browser-e2e-media  # Browser publish/subscribe
```

## Relay E2E

For local two-relay testing, build the shared relay image and generate certificates once, then start the stack (relays on ports 4433 and 4434 sharing a VTS and Redis route registry):

```shell
make relay-certs && docker compose build relay-common && docker compose up -d
```

Each suite directory contains a client crate and a `run.sh` that starts the stack, runs the test, and tears it down:

| Script | Checks |
| --- | --- |
| `./tests/auth-e2e/run.sh` | JWT acceptance, expiry, and token refresh across two relays and VTS |
| `./tests/fetch-e2e/run.sh` | `FETCH` range resolution against the relay cache |
| `./tests/cache-eviction-e2e/run.sh` | Object TTL eviction and idle track reclamation |
| `./tests/cascading-relay-e2e/run.sh` | Publish on `relay-a`, subscribe/fetch on `relay-b` (draft-18, then draft-14) |
| `./tests/multiple-publishers-e2e/run.sh` | Second publisher on the same track is ignored |
| `./tests/object-dedup-e2e/run.sh` | Republishing a group does not duplicate objects for `FETCH` |
| `./tests/cross-version-e2e/run.sh` | Bidirectional relaying between draft-14 and draft-18 peers (local VTS/relay without Docker; `CROSS_VERSION_E2E_VTS_PORT=8081`, `CROSS_VERSION_E2E_RELAY_PORT=4633`) |
| `./tests/moqtsink-e2e/run.sh` | GStreamer `moqtsink` publishes expected tracks |
| `./tests/live-translate-e2e/run.sh` | `live-translate` catalog languages and source-aligned timestamps |
| `./tests/sip-gateway-e2e/run.sh` | SIP gateway bridges Asterisk (TLS + SDES-SRTP) calls to a MoQT room and the phone example places/answers calls in Chromium |

## Browser E2E

Playwright specs live in `examples/browser/tests`; runners in `tests/browser-e2e/` launch relay, VTS, and Vite automatically (`make browser-e2e-*` or `node tests/browser-e2e/run-*-e2e.mjs`).

| Runner | Extra services | Checks |
| --- | --- | --- |
| `run-media-e2e.mjs` (`make browser-e2e-media`) | — | WebCodecs publish/subscribe, rendition selection, mute/unmute, screen share |
| `run-message-e2e.mjs`, `run-datagram-e2e.mjs`, `run-whiteboard-e2e.mjs` | — | Control messages, datagrams, and shared whiteboard with `FETCH` catch-up |
| `run-meeting-e2e.mjs` (`make browser-e2e-meeting`) | Two relays (`relay-a`, `relay-b`) via Docker Compose | Multi-party meeting across relays, ABR, screen share, chat, auth rejection |
| `run-live-viewer-e2e.mjs` (`make browser-e2e-live-viewer`) | `ingest-gateway` + `ffmpeg` + `recorder` | Live ingest playback, rendition switch, relay-cache rewind, CMAF/MSE, archive replay |
| `run-goaway-e2e.mjs` | Two relays + Redis + `ingest-gateway` + `recorder` | Browser session migration on relay `GOAWAY` without playback interruption |
| `run-console-e2e.mjs` | VTS + console (temp registry) | Issued tokens work until secret rotation or app deletion without affecting other apps |
| `run-live-translate-e2e.mjs` (`make browser-e2e-live-translate`) | `live-translate --echo` | Viewer switches between translated and original audio in sync with video |
| `run-meeting-transcription-e2e.mjs` | `bot-dispatcher` + `temporary-apps` + `moq-meeting-transcriber` | On-demand transcriber bot publishes captions over event timeline |
| `run-robot-teleop-e2e.mjs` | `moq-robot-sim` + `moq-robot-policy` + `recorder` + `bot-dispatcher` | Robot teleoperation, policy handover, dead-man hold, and episode recording |
| `run-chat-moderation-e2e.mjs`, `run-camera-detection-e2e.mjs` | Fake System One server + `bot-dispatcher` + `temporary-apps` + Python bot | On-demand chat moderation and camera detection bots |
| `run-ptz-tracking-e2e.mjs` | Fake ONVIF camera (`fake-ingest-gateway-pull.py`) + fake System One server | PTZ tracking bot sends `ContinuousMove` then `Stop` for detected boxes |

Override default ports with `RELAY_PORT`, `VTS_PORT`, or `MEDIA_E2E_WEB_PORT`.

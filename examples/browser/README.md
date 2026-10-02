# browser examples

Browser demos built on `bindings/wasm`.

```shell
npm --prefix examples/browser ci
make browser        # wasm build + Vite on http://localhost:5173
make chrome         # Chrome trusting the local relay certificate (make chrome:linux on Linux)
```

## Examples

| Page | Shows |
| --- | --- |
| `message` | Sending and receiving MoQT control messages by hand |
| `media` | Publisher and subscriber pages exchanging WebCodecs media |
| `media-cmaf` | The same with CMAF packaging |
| `webcodecs` | Encoder and decoder behaviour without MoQ in between |
| [`live-viewer`](examples/live-viewer/README.md) | Player for `live-ingest`, `moqtsink` and in-browser MP4 publishing, with rewind |
| [`meeting`](examples/meeting/README.md) | Multi-party video meeting |
| `onvif` | Remote control and monitoring of a PTZ camera through `bridges/onvif` |
| `remote-monitoring` | Remote monitoring from a USB camera |
| `moq-chat-moderation` | Chat moderated by a pipecat bot, see [`examples/python`](../python/moq-chat-moderation/README.md) |
| `moq-camera-detection` | Questions about a camera feed answered by a pipecat bot, see [`examples/python`](../python/moq-camera-detection/README.md) |

## Relay and authentication

Relays always authenticate. A session without a token is limited to
`anon/**`, so every example connects tokenless and defaults to namespaces
such as `anon/live/test`; keep that prefix when typing your own.

The meeting example also accepts an app-scoped JWT via `?jwt=<token>` (minted
with `services/vts/bin/mint.mjs`). Its namespace root then becomes the token's
appId, and a rejected token is reported with the relay's close code and reason.

## E2E

```shell
make browser-e2e-media
make browser-e2e-live-viewer
make browser-e2e-meeting
node scripts/run-message-e2e.mjs
node scripts/run-chat-moderation-e2e.mjs
node scripts/run-camera-detection-e2e.mjs
```

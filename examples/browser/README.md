# browser examples

Browser demos built on `sdk/js`.

```shell
npm ci              # repository root is an npm workspace
make browser        # build wasm + start Vite on http://localhost:5173
make chrome         # Chrome trusting the local relay cert (make chrome:linux on Linux)
```

`make chrome` uses fake camera/mic; run `MOQT_FAKE_MEDIA=0 make chrome` for real devices.

## Examples

| Page                                                                                                                                                                                                                                                         | Description                                                                    |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------ |
| [`message`](examples/message/README.md)                                                                                                                                                                                                                      | Send and receive MoQT control messages manually                                |
| [`datagram`](examples/datagram/README.md)                                                                                                                                                                                                                    | Compare Subgroup streams vs unreliable Datagrams                               |
| [`whiteboard`](examples/whiteboard/README.md)                                                                                                                                                                                                                | Shared whiteboard with `FETCH` catch-up                                        |
| [`media`](examples/media/README.md), [`media-cmaf`](examples/media-cmaf/README.md)                                                                                                                                                                           | WebCodecs publisher/subscriber in LOC or CMAF packaging                        |
| [`embed`](examples/embed/README.md)                                                                                                                                                                                                                          | Playback using the `<moq-player>` custom element (`moq-wasm/element`)          |
| [`live-viewer`](examples/live-viewer/README.md)                                                                                                                                                                                                              | Player with rewind for `ingest-gateway`, `moqtsink`, and in-browser MP4        |
| [`meeting`](examples/meeting/README.md)                                                                                                                                                                                                                      | Multi-party video meeting                                                      |
| [`recording-player`](examples/recording-player/README.md)                                                                                                                                                                                                    | Replay finished MoQ recordings from `services/recorder` (`moq-wasm/recording`) |
| [`phone`](examples/phone/README.md)                                                                                                                                                                                                                          | Inbound/outbound SIP calling via `services/sip-gateway` (`PhoneCall`)          |
| [`live-translate`](examples/live-translate/README.md)                                                                                                                                                                                                        | Live translated broadcast via `examples/rust/live-translate`                   |
| [`onvif`](examples/onvif/README.md)                                                                                                                                                                                                                          | Remote PTZ camera control and viewing via `services/ingest-gateway`            |
| [`remote-monitoring`](examples/remote-monitoring/README.md)                                                                                                                                                                                                  | USB camera remote monitoring                                                   |
| [`robot-teleop`](examples/robot-teleop/README.md)                                                                                                                                                                                                            | Robot teleoperation (`moq-robot-sim` / `moq-robot-policy`) with dead-man hold  |
| [`observability`](examples/observability/README.md)                                                                                                                                                                                                          | Live relay and Pub/Sub topology viewer (`services/observability`)              |
| [`moq-chat-moderation`](examples/moq-chat-moderation/README.md), [`moq-camera-detection`](examples/moq-camera-detection/README.md), [`moq-ptz-tracking`](examples/moq-ptz-tracking/README.md), [`moq-ai-voice-agent`](examples/moq-ai-voice-agent/README.md) | Pipecat bot demos (see [`examples/python`](../python))                         |

## Relay and authentication

Use the tabs at the top right of the example list to pick an access mode (saved in `localStorage`):

| Mode                                     | Namespace root               | Token                                                                                                                    |
| ---------------------------------------- | ---------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| **一時トークン** (default when deployed) | `temp-…`                     | 1-hour token from `services/temporary-apps`, auto-refreshed and kept in `#jwt=<token>` so shared links join the same app |
| **Public** (default locally)             | `anon` (visible to everyone) | None                                                                                                                     |

- `?access=public` opens a single page in public mode without changing the stored mode.
- `?jwt=<token>` connects with an app-scoped JWT (from `auth-token` CLI or console); root is the token's `appId` and it is not refreshed.
- Bot demos launch bots via `services/temporary-apps` → `services/bot-dispatcher`, so bots only run in temporary-app mode ([design](../../docs/architecture/bot-dispatcher/architecture.md)).
- Query overrides (`?relayUrl=`, `?recorderUrl=`, `?temporaryAppsUrl=`) are only accepted for this machine (`localhost`, `127.0.0.1`, `[::1]`) or `*.moqt.research.skyway.io` since the page sends its token there.

## Safari

Playwright's WebKit lacks WebTransport; verify Safari manually on macOS:

1. Trust the relay certificate once:

   ```shell
   mkdir -p ~/.config/moqt/relay-keys && cp services/relay/keys/*.pem ~/.config/moqt/relay-keys/
   security add-trusted-cert -r trustRoot -p ssl -k ~/Library/Keychains/login.keychain-db ~/.config/moqt/relay-keys/cert.pem
   ```

2. Run `RELAY_CERT_DIR=~/.config/moqt/relay-keys make relay` and `make browser`.
3. Open `media/publisher` and `media/subscriber` under `http://localhost:5173/moq-wasm/examples/` and step through the buttons.

Safari allows only one tab to capture the camera at a time; use a second browser to test meetings.

## E2E

```shell
make browser-e2e-media
make browser-e2e-live-viewer
make browser-e2e-meeting
make browser-e2e-live-translate
node tests/browser-e2e/run-message-e2e.mjs
node tests/browser-e2e/run-chat-moderation-e2e.mjs
node tests/browser-e2e/run-camera-detection-e2e.mjs
```

See [`tests/README.md`](../../tests/README.md#browser-e2e) for the full list of browser E2E runners.

# browser examples

Browser demos built on `crates/wasm`.

```shell
npm --prefix examples/browser ci
make browser        # wasm build + Vite on http://localhost:5173
make chrome         # Chrome trusting the local relay certificate (make chrome:linux on Linux)
```

## Examples

| Page                                            | Shows                                                                                                                      |
| ----------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `message`                                       | Sending and receiving MoQT control messages by hand                                                                        |
| `media`                                         | Publisher and subscriber pages exchanging WebCodecs media                                                                  |
| `media-cmaf`                                    | The same with CMAF packaging                                                                                               |
| `webcodecs`                                     | Encoder and decoder behaviour without MoQ in between                                                                       |
| [`live-viewer`](examples/live-viewer/README.md) | Player for `live-ingest`, `moqtsink` and in-browser MP4 publishing, with rewind                                            |
| [`meeting`](examples/meeting/README.md)         | Multi-party video meeting                                                                                                  |
| `onvif`                                         | Remote control and monitoring of a PTZ camera through `crates/onvif-ingest`                                                |
| `remote-monitoring`                             | Remote monitoring from a USB camera                                                                                        |
| `moq-chat-moderation`                           | Chat moderated by a pipecat bot, see [`examples/python`](../python/moq-chat-moderation/README.md)                          |
| `moq-camera-detection`                          | Questions about a camera feed answered by a pipecat bot, see [`examples/python`](../python/moq-camera-detection/README.md) |
| `moq-ptz-tracking`                              | ONVIF camera kept on a target by a pipecat bot, see [`examples/python`](../python/moq-ptz-tracking/README.md)              |

## Relay and authentication

Relays always authenticate. A session without a token is limited to
`anon/**`, so every example connects tokenless and defaults to namespaces
such as `anon/live/test`; keep that prefix when typing your own.

The meeting example also accepts an app-scoped JWT via `?jwt=<token>` (signed
with the `auth-token` CLI). Its namespace root then becomes the token's
appId, and a rejected token is reported with the relay's close code and reason.

## Safari

Playwright's WebKit has no WebTransport, so check Safari on a Mac by hand.

1. Copy the relay certificate out of the checkout and trust it once; every
   checkout's relay reads it through `RELAY_CERT_DIR`:

   ```shell
   mkdir -p ~/.config/moqt/relay-keys && cp crates/relay/keys/*.pem ~/.config/moqt/relay-keys/
   security add-trusted-cert -r trustRoot -p ssl -k ~/Library/Keychains/login.keychain-db ~/.config/moqt/relay-keys/cert.pem
   ```

2. Start the relay with it and the pages: `RELAY_CERT_DIR=~/.config/moqt/relay-keys make relay` and `make browser`.
3. In Safari, open `media/publisher` and `media/subscriber` under
   `http://localhost:5173/moq-wasm/examples/` and go through each page's buttons
   in order. The subscriber draws on a canvas, since Safari has no
   `MediaStreamTrackGenerator`; its playback status stays `Playback idle`, as
   it follows the `<video>` element.

Safari lets one page capture the camera at a time, so a meeting check needs a
second browser for the other participant. With `safaridriver --enable` and
Develop > Allow Remote Automation, a WebDriver session gets mock camera and
microphone devices instead of the real ones.

## E2E

```shell
make browser-e2e-media
make browser-e2e-live-viewer
make browser-e2e-meeting
node tests/browser-e2e/run-message-e2e.mjs
node tests/browser-e2e/run-chat-moderation-e2e.mjs
node tests/browser-e2e/run-camera-detection-e2e.mjs
```

# browser examples

Browser demos built on `crates/wasm`.

```shell
npm ci              # the repository root is an npm workspace
make browser        # wasm build + Vite on http://localhost:5173
make chrome         # Chrome trusting the local relay certificate (make chrome:linux on Linux)
```

`make chrome` gives Chrome a fake camera and microphone;
`MOQT_FAKE_MEDIA=0 make chrome` uses the real ones.

## Examples

| Page                                                              | Shows                                                                               |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `message`                                                         | Sending and receiving MoQT control messages by hand                                 |
| `media`, `media-cmaf`                                             | Publisher and subscriber pages exchanging WebCodecs media, in LOC or CMAF packaging |
| `webcodecs`                                                       | Encoder and decoder behaviour without MoQ in between                                |
| [`live-viewer`](examples/live-viewer/README.md)                   | Player for `live-ingest`, `moqtsink` and in-browser MP4 publishing, with rewind     |
| [`meeting`](examples/meeting/README.md)                           | Multi-party video meeting                                                           |
| [`live-translate`](examples/live-translate/README.md)             | Broadcast translated live by `services/live-translate`                              |
| `onvif`                                                           | Remote control and monitoring of a PTZ camera through `services/onvif-ingest`       |
| `remote-monitoring`                                               | Remote monitoring from a USB camera                                                 |
| `moq-chat-moderation`, `moq-camera-detection`, `moq-ptz-tracking` | Pages backed by a pipecat bot, see [`examples/python`](../python)                   |

## Relay and authentication

The tabs at the top right of the example list choose how every example
connects; the choice is kept in localStorage.

| Mode                                                    | Namespace root                 | Token                                                                                                                                                  |
| ------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **一時トークン** (temporary), the default when deployed | `temp-…`, a new temporary app  | A one-hour token from `services/token-mint`, refreshed by the page. It stays in the address bar as `#jwt=<token>`, so a copied link joins the same app |
| **Public**, the default on this machine                 | `anon`, which everyone can see | None                                                                                                                                                   |

- `?access=public` opens one page as public without changing the stored mode.
- `?jwt=<token>` connects with any app-scoped JWT (from the `auth-token` CLI
  or the console). The root becomes the token's appId; the token is never
  refreshed.
- The bot-backed examples hand a temporary page's token to the bot service,
  so the bot joins the same app
  (`docs/architecture/ai-bots/on-demand-rooms.md`).
- A service URL in the query (`?relayUrl=`, `?recorderUrl=`, `?tokenMintUrl=`,
  `?botUrl=`) is used only when it names this machine or a host under
  `moqt.research.skyway.io`, since the page sends its token there.

## Safari

Playwright's WebKit has no WebTransport, so check Safari on a Mac by hand.

1. Trust the relay certificate once:

   ```shell
   mkdir -p ~/.config/moqt/relay-keys && cp services/relay/keys/*.pem ~/.config/moqt/relay-keys/
   security add-trusted-cert -r trustRoot -p ssl -k ~/Library/Keychains/login.keychain-db ~/.config/moqt/relay-keys/cert.pem
   ```

2. Run `RELAY_CERT_DIR=~/.config/moqt/relay-keys make relay` and `make browser`.
3. Open `media/publisher` and `media/subscriber` under
   `http://localhost:5173/moq-wasm/examples/` and go through each page's
   buttons in order.

Safari lets one page capture the camera at a time, so a meeting check needs a
second browser for the other participant.

## E2E

```shell
make browser-e2e-media
make browser-e2e-live-viewer
make browser-e2e-meeting
node tests/browser-e2e/run-message-e2e.mjs
node tests/browser-e2e/run-chat-moderation-e2e.mjs
node tests/browser-e2e/run-camera-detection-e2e.mjs
```

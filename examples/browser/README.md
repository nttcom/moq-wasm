# browser examples

Browser demos built on `crates/wasm`.

```shell
npm ci              # the repository root is an npm workspace
make browser        # wasm build + Vite on http://localhost:5173
make chrome         # Chrome trusting the local relay certificate (make chrome:linux on Linux); MOQT_FAKE_MEDIA=0 for the real camera and microphone
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
| `onvif`                                         | Remote control and monitoring of a PTZ camera through `services/onvif-ingest`                                              |
| `remote-monitoring`                             | Remote monitoring from a USB camera                                                                                        |
| `moq-chat-moderation`                           | Chat moderated by a pipecat bot, see [`examples/python`](../python/moq-chat-moderation/README.md)                          |
| `moq-camera-detection`                          | Questions about a camera feed answered by a pipecat bot, see [`examples/python`](../python/moq-camera-detection/README.md) |
| `moq-ptz-tracking`                              | ONVIF camera kept on a target by a pipecat bot, see [`examples/python`](../python/moq-ptz-tracking/README.md)              |

## Relay and authentication

Relays always authenticate. The tabs at the top right of the example list
choose how every example connects, and the choice is kept in localStorage:

- **一時トークン** (temporary): the page gets a one-hour token for a new
  temporary app from `services/token-mint` and uses `temp-…` as its
  namespace root, so only the people a page's link reaches join its rooms or
  see its recordings. The current token stays in the address bar as
  `#jwt=<token>`, so a copied link (or 共有リンクをコピー) joins the same app;
  opening one switches the visitor to that app. Pages refresh the token
  before it expires; a link stops working an hour after its token was signed.
  The issuer is `?tokenMintUrl=`, else `http://localhost:8083` for a page
  served from this machine and the deployed one otherwise.
- **Public**: tokenless, limited to `anon/**`, which everyone can see.

A page served from this machine starts public, a deployed one temporary,
until a tab is chosen. Namespace inputs default under the page's root, for
example `anon/live/test` or `temp-…/live/test`; keep that root when typing
your own. `utils/access.ts` resolves the mode once per page, and
`utils/accessBar.ts` shows it at the top of every example.

`?jwt=<token>` connects with any app-scoped JWT instead (for example one
signed with the `auth-token` CLI or by the console). Its namespace root then
becomes the token's appId; the token is used as is and never refreshed.

The bot-backed examples (chat moderation, camera detection, PTZ tracking and
the Meeting captions) hand a temporary page's token to the bot service in the
`Authorization` header of the room request (`utils/botRoom.ts`), and the bot
joins the room under the same app, refreshing its own copy of the token on
the token mint (docs/architecture/ai-bots/on-demand-rooms.md "Temporary
apps"). A public page's request carries no token, and its bot runs under
`anon`. A `?jwt=` token is not handed over, since the issuer cannot refresh
it: those pages' bots stay under `anon`, and the Meeting hides its captions.

A URL in a page's query, such as `?relayUrl=`, `?recorderUrl=`,
`?tokenMintUrl=` or `?botUrl=`, is used only when it names this machine
(`localhost`, `127.0.0.1`, `[::1]`) or a host under `moqt.research.skyway.io`.
A page sends its token to the URLs it uses, so it ignores a URL on any other
host (`urlFromQuery` in `utils/serviceUrl.ts`).

## Safari

Playwright's WebKit has no WebTransport, so check Safari on a Mac by hand.

1. Copy the relay certificate out of the checkout and trust it once; every
   checkout's relay reads it through `RELAY_CERT_DIR`:

   ```shell
   mkdir -p ~/.config/moqt/relay-keys && cp services/relay/keys/*.pem ~/.config/moqt/relay-keys/
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

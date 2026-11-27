# moq-ptz-tracking

Watches an ONVIF PTZ camera stream (`services/ingest-gateway`) and pairs with the Python tracking bot ([`examples/python/moq-ptz-tracking`](../../../python/moq-ptz-tracking/README.md)) to detect targets, draw bounding boxes, and steer the camera automatically.

## Run

1. Start a relay (`make relay`), stream your ONVIF camera via `ingest-gateway`:
   ```shell
   make onvif \
     ONVIF_AUTH_TOKEN=<token> \
     ONVIF_PUBLISH_NAMESPACE=<appId>/moq-ptz-tracking/onvif/client \
     ONVIF_SUBSCRIBE_NAMESPACE=<appId>/moq-ptz-tracking/bots/ptz-tracking/command
   ```
   Register `ptz-tracking` in [`services/bot-dispatcher`](../../../../services/bot-dispatcher/README.md) (see [`examples/python/moq-ptz-tracking/README.md`](../../../python/moq-ptz-tracking/README.md)), and run `make browser` + `make chrome`.
2. Open `http://localhost:5173/moq-wasm/examples/moq-ptz-tracking/` in temporary-app mode (**一時トークン**).
3. Click **Join**, click **bot を参加させる**, enter one or more targets (up to 16), and click **追従** (or **停止** to pause tracking).

## Tracks

- Room namespace: `<root>/moq-ptz-tracking`.
- Camera stream: `<room>/onvif/client` (played with `LivePlayer`).
- Tracking prompt: page publishes JSON `{targets, video}` on `<room>/page/viewer` (track `prompt`).
- Bot (`ptz-tracking`) publishes detection boxes and pan/tilt durations on its timeline track and sends PTZ commands to `<room>/bots/ptz-tracking/command`.

## E2E

```shell
node tests/browser-e2e/run-ptz-tracking-e2e.mjs
```

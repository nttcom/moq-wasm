# moq-camera-detection

Streams the browser camera to the Python vision bot ([`examples/python/moq-camera-detection`](../../../python/moq-camera-detection/README.md)) and renders real-time multiple-choice verdicts and bounding boxes over the video.

## Run

1. Start a relay (`make relay`), register `camera-detection` in [`services/bot-dispatcher`](../../../../services/bot-dispatcher/README.md) (see [`examples/python/moq-camera-detection/README.md`](../../../python/moq-camera-detection/README.md)), and run `make browser` + `make chrome`.
2. Open `http://localhost:5173/moq-wasm/examples/moq-camera-detection/` in temporary-app mode (**一時トークン**).
3. Click **Join** to publish your camera, click **bot を参加させる**, and edit up to 8 questions (multiple-choice or bounding-box targets) then click **適用**.

## Tracks

- Room namespace: `<root>/moq-camera-detection/<room>` (override room with `?room=`).
- Page publishes H.264 video (640×480) and a JSON `prompt` track under `<room>/camera`.
- Bot (`camera-detection`) publishes answers and bounding boxes on its event timeline track.

## E2E

```shell
node tests/browser-e2e/run-camera-detection-e2e.mjs
```

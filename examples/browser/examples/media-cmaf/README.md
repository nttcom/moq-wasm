# media-cmaf

Two-page publisher and subscriber demo publishing LOC and CMAF tracks side by side (`cmaf: true` in `createBroadcast`) and playing CMAF video via Media Source Extensions (MSE).

## Run

```shell
make relay
make browser
make chrome
```

1. Open `http://localhost:5173/moq-wasm/examples/media-cmaf/publisher/`, connect to a relay, start camera capture, optionally adjust keyframe interval, and click **Publish**.
2. Copy the Track Namespace.
3. Open `http://localhost:5173/moq-wasm/examples/media-cmaf/subscriber/` in another tab, connect to the same relay, paste the Track Namespace, and click **Subscribe** to play the CMAF video track via `<moq-player>`.

## Notes

- Audio has no CMAF track in this demo, so the subscriber plays video only.
- Covered by the media E2E suite (`make browser-e2e-media`).

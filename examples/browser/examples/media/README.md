# media

Two-page WebCodecs publisher and subscriber demo using MSF catalogs and LOC packaging (`moq-wasm/media` and `<moq-player>`).

## Run

```shell
make relay
make browser
make chrome
```

1. Open `http://localhost:5173/moq-wasm/examples/media/publisher/`, connect to a relay, start camera/mic capture, and click **Publish**.
2. Copy the Track Namespace.
3. Open `http://localhost:5173/moq-wasm/examples/media/subscriber/` in another tab, connect to the same relay, paste the Track Namespace, load the catalog, pick video/audio tracks, and subscribe.

## Features

- **Publisher**: Publishes camera (1080p main + 1/4-scale `low` rendition at 150 kbps / 15 fps) and audio with on-demand encoding (unsubscribed tracks are not encoded), video mute, and optional screen share (`screen`).
- **Subscriber**: Inspects the broadcast's `catalog` track, lets you choose video and audio tracks manually, and plays them in `<moq-player>` with live playout stats.

## E2E

```shell
make browser-e2e-media
```

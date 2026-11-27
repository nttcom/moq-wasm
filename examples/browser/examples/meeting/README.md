# Meeting

Multi-party browser meeting with camera, screen share, audio, chat, captions, and recording.

## Run

Run each command in a separate terminal:

```shell
make relay
make browser
make chrome
```

Open `http://localhost:5173/moq-wasm/examples/meeting/` in two tabs, select the local relay, and join the same room with different names. See [relay and authentication](../../README.md#relay-and-authentication) for access modes.

## Features

- **Rooms & layout**: Subscribes to everyone's audio and visible tiles' video. Switch between equal gallery and speaker view (screen share or loudest speaker via LOC Audio Level `draft-ietf-moq-loc-04 §2.3.3.2`).
- **Publishing**: Camera/screen share at 1080p/720p/480p and Opus audio (64 kbps, `camera_audio`), encoded only while subscribed. Device settings adjust capture constraints and hardware acceleration; clicking a remote participant opens per-track selection, playout buffer controls, and stats.
- **Playback & ABR**: Plays each remote participant via `RemoteBroadcast` (`moq-wasm/media`) with an adaptive jitter buffer and `Auto` (default) rendition selection based on tile size and delivery lag (switches at group boundaries).
- **Stats**: Per-participant modal charting bitrate, buffer, capture-to-display delay, and A/V offset.
- **Recording & captions**: In temporary-app rooms, **Activities** starts the [recorder](../../../../services/recorder/README.md) (MoQ archive with chat and transcript, composite MP4, or one file per participant or per track; `make recorder-serve` lists recordings and finished MoQ archives open in [Recording Player](../recording-player/)) or [transcriber](../../../python/moq-meeting-transcriber/README.md) via `temporary-apps` → [`bot-dispatcher`](../../../../services/bot-dispatcher/README.md). Override URLs with `?temporaryAppsUrl=` and `?recorderUrl=`.
- **Chat**: Sidebar text chat.

Note: Recording start requires a temporary app (registered apps start bots from their own server); listing recordings requires a token (temporary app or `?jwt=`) whose `subscribe` claim covers the room; the transcriber can join public and temporary-app rooms (not `?jwt=`), though only a temporary-app page starts it.

## Protocol flow

| Item            | Name                                                          |
| --------------- | ------------------------------------------------------------- |
| Track namespace | `<root>/<room>/<user>` (`anon` or app ID)                     |
| Catalog         | `catalog`                                                     |
| Camera          | `camera` (1080p), `camera_720p`, `camera_480p`                |
| Screen share    | `screenshare` (1080p), `screenshare_720p`, `screenshare_480p` |
| Audio           | `camera_audio`                                                |
| Chat            | `chat`                                                        |

1. `SUBSCRIBE_NAMESPACE` on `<root>/<room>` discovers current and future participants via `PUBLISH_NAMESPACE`.
2. `createBroadcast()` announces `<root>/<room>/<user>` and `PUBLISH`es `catalog` plus listed tracks; muting drops the track with `PUBLISH_DONE`.
3. Receivers open one catalog `SUBSCRIBE` per participant, then per-track `SUBSCRIBE`/`UNSUBSCRIBE` for selected media (using joining `FETCH` to start video at the current group's keyframe).

## Stack

React 19, TypeScript, Tailwind CSS, shadcn/ui, Vite, `sdk/js`.

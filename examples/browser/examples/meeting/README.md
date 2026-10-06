# Meeting

Multi-party video meeting over MoQT. Every participant publishes an MSF
catalog with camera, screen-share and microphone tracks under
`/{RoomName}/{UserName}` and subscribes to the tracks of the others it picks.

## Run

```shell
make relay
make browser
make chrome
```

Open `http://localhost:5173/examples/meeting/`, choose a relay, enter a Room
Name and a User Name and join. `?jwt=<token>` connects with an app-scoped JWT
(see [`examples/browser/README.md`](../../README.md)).

## Features

- **Rooms**: participants are discovered through `SUBSCRIBE_NAMESPACE` on `/{RoomName}/`. Joining subscribes to nothing; media is chosen per participant.
- **Publishing**: camera, microphone and screen share. Enabling a source adds three catalog profiles (camera and screen share: 1080p / 720p / 480p; audio: 128 / 64 / 32 kbps), each encoded by its own worker with the codec and bitrate of the profile. Codecs include H.264 High@5.0 and AV1.
- **Catalog editing**: `Catalog Details` adds, removes and edits tracks from presets; each video track has its own keyframe interval; each audio track sends either one stream or a new group every N seconds (default 1 s). Disabling a source keeps its tracks, since disabled may mean muted.
- **Subscribing**: `Catalog Subscribe` on a participant card lists their tracks; `Subscribe Video` / `Subscribe Audio` pick one of each. The decoder is configured from the catalog codec and catalog updates are applied live.
- **Playback**: each remote participant plays through the Live Player's live pipeline (`lib/player`, `LocLive`): one playout clock keeps the picture and the sound together behind an adaptive jitter buffer, and the sound is scheduled on an `AudioContext`. The playout settings (Min / Max buffer, Catch up) are per participant, as in the Live Viewer.
- **Stats**: a per-participant modal charts bitrate, the playout buffer against its target, the capture-to-display delay (from the LOC capture timestamp) and the A/V offset. Each remote video shows a stats line with the same numbers.
- **Chat** sidebar.

## Protocol flow

| Item            | Name                                                        |
| --------------- | ----------------------------------------------------------- |
| Track namespace | `/{RoomName}/{UserName}`                                    |
| Catalog         | `catalog`                                                   |
| Camera          | `camera_1080p`, `camera_720p`, `camera_480p`                |
| Screen share    | `screenshare_1080p`, `screenshare_720p`, `screenshare_480p` |
| Audio           | `audio_128kbps`, `audio_64kbps`, `audio_32kbps`             |

1. `CLIENT_SETUP` / `SERVER_SETUP` with the relay.
2. `SUBSCRIBE_NAMESPACE` on `/{RoomName}/`; `PUBLISH_NAMESPACE` arrives for every present and later participant.
3. `PUBLISH_NAMESPACE` for `/{RoomName}/{UserName}`. The catalog object is sent on each catalog `SUBSCRIBE` and again whenever it changes.
4. Media `SUBSCRIBE`s from others are answered on the alias of `SUBSCRIBE_OK`; audio tracks in group-update mode start a new group and subgroup stream every N seconds and close the previous one with EndOfGroup.
5. Receiving mirrors this: catalog `SUBSCRIBE`, then per-track `SUBSCRIBE` and `UNSUBSCRIBE`; audio playback is reset on resubscribe.

## Stack

React 19, TypeScript, Tailwind CSS, shadcn/ui, Vite, `crates/wasm`.

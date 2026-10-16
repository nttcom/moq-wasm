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

- **Rooms**: participants are discovered through `SUBSCRIBE_NAMESPACE` on `/{RoomName}/`. Every participant's audio is subscribed, and their video while their tile is on the stage.
- **Layout**: a gallery of equal tiles, or a speaker view with the active speaker large and everyone else in a filmstrip on the right. Closing the filmstrip unsubscribes the video of the participants it hid. A screen share takes the focus. The bottom bar holds the microphone, camera and screen share toggles, device and catalog settings, the view and filmstrip toggles, the chat and Leave; a tile's `⋯` opens its tracks, playout settings and stats.
- **Active speaker**: audio objects carry the LOC Audio Level (RFC 6464 level and voice activity, draft-ietf-moq-loc-01 §2.3.3.1). The loudest participant speaking takes the focus once the current speaker has been quiet for 1.5 s, and a speaking participant's tile and header chip are ringed.
- **Publishing**: camera, microphone and screen share. Enabling a source adds three catalog profiles (camera and screen share: 1080p / 720p / 480p; audio: 128 / 64 / 32 kbps), each encoded by its own worker with the codec and bitrate of the profile, and only while someone subscribes to it. Codecs include H.264 High@5.0 and AV1. Video sends a keyframe every 2 s by default and numbers each group by the capture time of its keyframe, so the groups of different profiles are ordered by media time.
- **Catalog editing**: `Catalog Details` adds, removes and edits tracks from presets; each video track has its own keyframe interval; each audio track sends either one stream or a new group every N seconds (default 1 s). Disabling a source keeps its tracks, since disabled may mean muted.
- **Tracks**: a tile's details list the participant's tracks and which one plays. The decoder is configured from the catalog codec and catalog updates are applied live.
- **Quality switching**: the track selects stay usable while subscribed. A track of the same codec takes over the picture at its first keyframe while the previous one plays on, then the previous one is unsubscribed; another codec resubscribes. `Auto` (the default) picks among the same-codec tracks itself: it starts at the tallest one the tile needs, follows the tile when the view changes (at once, unless the path was congested in the last 10 s), goes lower when the capture-to-arrival lag rises, and one step higher after a calm spell, as in the Live Viewer. Picking a track turns `Auto` off.
- **Playback**: each remote participant plays through the Live Player's live pipeline (`lib/player`, `LocLive`): one playout clock keeps the picture and the sound together behind an adaptive jitter buffer, and the sound is scheduled on an `AudioContext`. The playout settings (Min / Max buffer, Catch up) are per participant, as in the Live Viewer.
- **Stats**: a per-participant modal charts bitrate, the playout buffer against its target, the capture-to-display delay (from the LOC capture timestamp) and the A/V offset. Each remote video shows a stats line with the same numbers.
- **Recording**: with a recorder (`cargo run -p recorder -- serve --relay <relay URL>`, see [`crates/recorder`](../../../../crates/recorder/README.md)), the record button in the bottom bar records the whole room, either each participant's video and audio in one file or one file per track. Every participant sees `REC` in the header while it runs. The page uses the recorder at `?recorderUrl=`, else `http://localhost:8096` for a local relay and `https://recorder.moqt.research.skyway.io` for any other; the button appears once that recorder answers.
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
5. Receiving mirrors this: catalog `SUBSCRIBE`, then per-track `SUBSCRIBE` and `UNSUBSCRIBE`; audio playback is reset on resubscribe. A quality switch sends the `SUBSCRIBE` of the new track first and the `UNSUBSCRIBE` of the old one once the new one has taken over. A viewer joining a track the relay already receives starts at its next keyframe, since draft-14 gives the relay no way to ask the publisher for one.

## Stack

React 19, TypeScript, Tailwind CSS, shadcn/ui, Vite, `crates/wasm`.

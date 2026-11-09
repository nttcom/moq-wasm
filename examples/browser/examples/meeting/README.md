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
- **Layout**: a gallery of equal tiles, or a speaker view with the active speaker large and everyone else in a filmstrip on the right. The button at the top of the filmstrip closes and reopens it; closing it unsubscribes the video of the participants it hid. A screen share takes the focus. The bottom bar holds the catalog and stats, the microphone and camera toggles with a menu beside each that switches the device and opens the device settings, screen share, Leave, Activities, which holds Replay, Record and STT (captions), the view toggle and the chat; a tile's `⋯` opens its tracks, playout settings and stats.
- **Active speaker**: audio objects carry the LOC Audio Level (RFC 6464 level and voice activity, draft-ietf-moq-loc-04 §2.3.3.2). The loudest participant speaking takes the focus once the current speaker has been quiet for 1.5 s, and a speaking participant's tile and header chip are ringed.
- **Publishing**: camera, microphone and screen share. Enabling a source adds three catalog profiles (camera and screen share: 1080p / 720p / 480p; audio: 128 / 64 / 32 kbps, of which a viewer subscribes to 32 kbps unless it picks another), each encoded by its own worker with the codec and bitrate of the profile, and only while someone subscribes to it. Codecs include H.264 High@5.0 and AV1. Video sends a keyframe every 2 s by default and numbers each group by the capture time of its keyframe, so the groups of different profiles are ordered by media time.
- **Catalog editing**: `Catalog Details` adds, removes and edits tracks from presets; each video track has its own keyframe interval; each audio track sends either one stream or a new group every N seconds (default 1 s). Disabling a source keeps its tracks, since disabled may mean muted.
- **Playback**: each remote participant plays through two `LivePlayer`s (`moq-wasm/media`), one for the camera and the microphone and one for the screen share, which share the participant's `BroadcastCatalog`, so the catalog is subscribed once. One playout clock keeps the picture and the sound together behind an adaptive jitter buffer, and the sound is scheduled on an `AudioContext`. The playout settings (Min / Max buffer, Catch up) are per participant, as in the Live Viewer; Min buffer defaults to 0 ms, so the buffer is only the measured jitter plus the output latency and a lower rendition shortens the delay.
- **Tracks**: a tile's details list the participant's tracks and which one plays; catalog updates are applied live.
- **Quality switching**: the track selects stay usable while subscribed. A rendition of the same switching set takes over the picture at a group boundary while the previous one plays on. `Auto` (the default) is the Live Player's choice: no taller than the tile, lower when the delivery falls behind, and one step higher after a calm spell. Picking a track turns `Auto` off.
- **Stats**: a per-participant modal charts bitrate, the playout buffer against its target, the capture-to-display delay (from the LOC capture timestamp) and the A/V offset. Each remote video shows a stats line with the same numbers.
- **Recording**: with a recorder (`make recorder-serve`, see [`services/recorder`](../../../../services/recorder/README.md)), Record in Activities, or the sidebar's Record tab, records the whole room: the MoQ archive with the chat and the transcript (`Record meeting with chat and transcript`), everyone in one video (`All Track in one file`), which a recorder started with `--composite` composites into `composite.mp4` once the recording stops, each participant's video and audio in one file (`User Track in one file`), or one file per track (`Per Track in one file`). `make recorder-serve` keeps the files in `recordings/store/<recording id>/`. Every participant sees `Starting` in the header until the recording job has joined the room, then `REC` while it runs. The page uses the recorder at `?recorderUrl=`, else `http://localhost:8096` for a local relay and `https://recorder.moqt.research.skyway.io` for any other; the button appears once that recorder answers. A finished MoQ archive opens in the [Recording Player](../recording-player/) from Replay in Activities (the latest one) or from the Record tab (every one).
- **Captions**: with the transcriber (`examples/python/moq-meeting-transcriber`), STT in Activities, or the sidebar's Transcription tab, asks it into the room, with Whisper or Granite Speech 470m on the CPU, or Qwen3-ASR or Granite Speech 2b on a GPU service, and keeps it there until someone stops it. It transcribes every participant; the newest utterance of a speaker shows at the bottom of their tile, and the Transcription tab lists everything said while the transcriber is in the room. Every participant sees `CC` in the header while it is in the room, and `CC starting` until a GPU service that scaled to zero answers. The page uses the transcriber at `?transcriberUrl=`, else `http://127.0.0.1:8100` for a local relay and `https://moq-meeting-transcriber.moqt.research.skyway.io` for any other. Rooms outside the public `anon` root (temporary mode or `?jwt=`) have no captions.
- **Chat** sidebar.

## Protocol flow

| Item            | Name                                                          |
| --------------- | ------------------------------------------------------------- |
| Track namespace | `/{RoomName}/{UserName}`                                      |
| Catalog         | `catalog`                                                     |
| Camera          | `video` (1080p), `video_720p`, `video_480p`                   |
| Screen share    | `screenshare` (1080p), `screenshare_720p`, `screenshare_480p` |
| Audio           | `audio`                                                       |
| Chat            | `chat`                                                        |

1. `CLIENT_SETUP` / `SERVER_SETUP` with the relay.
2. `SUBSCRIBE_NAMESPACE` on `/{RoomName}/`; `PUBLISH_NAMESPACE` arrives for every present and later participant.
3. `publish()` announces `/{RoomName}/{UserName}` and `PUBLISH`es the catalog and every track it lists; a muted device's track leaves the catalog with `PUBLISH_DONE`. The renditions of the camera and of the screen share are encoded from the same frames and share group ids; audio follows the video's groups, or starts a group every second while the camera is off.
4. Receiving mirrors this: one catalog `SUBSCRIBE` per participant, then the players' per-track `SUBSCRIBE` and `UNSUBSCRIBE`. A quality switch sends the `SUBSCRIBE` of the new track first and the `UNSUBSCRIBE` of the old one once the new one has taken over. A new picture starts at the current group's keyframe through a joining `FETCH`; a track that takes over a playing picture starts at its next group.

## Stack

React 19, TypeScript, Tailwind CSS, shadcn/ui, Vite, `crates/wasm`.

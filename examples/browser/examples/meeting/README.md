# Meeting

Multi-party video meeting over MoQT. Every participant publishes an MSF
catalog with camera, screen-share and microphone tracks under
`/{RoomName}/{UserName}` and subscribes to the tracks of the others.

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

- **Rooms**: participants are discovered through `SUBSCRIBE_NAMESPACE` on `/{RoomName}/`. Everyone's audio is subscribed, and their video while their tile is on the stage.
- **Layout**: a gallery of equal tiles, or a speaker view with the active speaker large and the others in a filmstrip. A screen share takes the focus.
- **Active speaker**: audio objects carry the LOC Audio Level (draft-ietf-moq-loc-04 §2.3.3.2); the loudest speaking participant takes the focus.
- **Publishing**: camera and screen share in 1080p / 720p / 480p, audio at 128 / 64 / 32 kbps. Each profile is encoded only while someone subscribes to it. `Catalog Details` edits the tracks, codecs and keyframe intervals.
- **Playback**: each remote participant plays through a `RemoteBroadcast` (`moq-wasm/media`) behind an adaptive jitter buffer, with playout settings per participant.
- **Quality switching**: `Auto` (the default) picks a rendition no taller than the tile and steps down when delivery falls behind; a new rendition takes over at a group boundary.
- **Stats**: a per-participant modal charts bitrate, the playout buffer, the capture-to-display delay and the A/V offset.
- **Recording**: with a recorder (`make recorder-serve`, see [`services/recorder`](../../../../services/recorder/README.md)), Record in Activities records the whole room as a MoQ archive with chat and transcript, as one composited video, or as one file per participant or per track. A finished MoQ archive opens in the [Recording Player](../recording-player/). `?recorderUrl=` names the recorder.
- **Captions**: with the transcriber ([`examples/python/moq-meeting-transcriber`](../../../python/moq-meeting-transcriber/README.md)), STT in Activities asks it into the room; each speaker's newest utterance shows on their tile. `?transcriberUrl=` names the transcriber.
- **Chat** sidebar.

A deployed recorder needs a token (a temporary app or `?jwt=`); the
transcriber joins public and temporary-app rooms, not `?jwt=` ones.

## Protocol flow

| Item            | Name                                                          |
| --------------- | ------------------------------------------------------------- |
| Track namespace | `/{RoomName}/{UserName}`                                      |
| Catalog         | `catalog`                                                     |
| Camera          | `camera` (1080p), `camera_720p`, `camera_480p`                |
| Screen share    | `screenshare` (1080p), `screenshare_720p`, `screenshare_480p` |
| Audio           | `camera_audio`                                                |
| Chat            | `chat`                                                        |

1. `SUBSCRIBE_NAMESPACE` on `/{RoomName}/`; `PUBLISH_NAMESPACE` arrives for every present and later participant.
2. `createBroadcast()` announces `/{RoomName}/{UserName}` and `PUBLISH`es the catalog and every track it lists; a muted device's track leaves the catalog with `PUBLISH_DONE`.
3. Receiving mirrors this: one catalog `SUBSCRIBE` per participant, then the players' per-track `SUBSCRIBE` and `UNSUBSCRIBE`. A new picture starts at the current group's keyframe through a joining `FETCH`.

## Stack

React 19, TypeScript, Tailwind CSS, shadcn/ui, Vite, `crates/wasm`.

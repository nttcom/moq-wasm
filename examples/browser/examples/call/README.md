# MoQT Video Call Application

Video call application using MoQT (Media over QUIC Transport).

## Overview

This application provides real-time video calling using the MoQT protocol defined by draft-ietf-moq-transport-14.

### Main Features

- **Room and user management**: join a room by entering a Room Name and User Name
- **Relay selection**: choose either `127.0.0.1` or a domain-based MoQT relay
- **Catalog-based subscription**: resolve video/audio tracks from the `catalog` track and subscribe to them
- **Dynamic catalog composition**: the initial Catalog is empty; default tracks are added when camera, mic, or screen share is enabled, and disabling a source does not remove its tracks automatically
- **Catalog profiles**: camera has 1080p/720p/480p profiles, audio has 128/64/32 kbps profiles, and screen share has 1080p/720p/480p profiles
- **Participant grid**: list other participants in the room
- **Selective subscription**: after `Catalog Subscribe`, select video/audio tracks and subscribe with `Subscribe Video` / `Subscribe Audio`
- **Media publishing**: publish selected camera, microphone, and screen share streams
- **Encoding settings**: choose codecs including H.264 High@5.0 (`avc1.640032`)
- **Separated settings modals**: `Device` and `Catalog` buttons open separate `Select Devices` and `Catalog Details` modals
- **Catalog track editing**: add, delete, and edit tracks from `Catalog Details`; tracks are grouped by `Video / Screenshare / Audio`, and codec/bitrate/resolution/channel values are selected from presets
- **Per-track keyframe settings**: configure `keyframeInterval` independently for each video and screen share track
- **Catalog detail display**: the `Catalogs` modal shows codec, bitrate, resolution, sample rate, channel, and live status for each track
- **Optimized debug logging**: subscriber/decoder logs focus on first receive events, configuration changes, and warnings, while high-volume per-object logs are suppressed
- **AV1 initialization fix**: video decoder initialization prefers the Catalog codec and does not incorrectly fall back to `avc1` for AV1 tracks
- **Decoder initialization policy**: Catalog information is fixed at first decoder initialization; dynamic reinitialization during decode and default codec fallback are avoided
- **Independent encoding resolution**: Catalog-based encoding resolution treats `camera` and `screenshare` independently so one track setting does not leak into the other
- **Codec application on subscribe**: the subscriber applies the selected Catalog track codec to decoder initialization, including screen-share-only subscriptions
- **Profile application on subscribe**: the publisher applies the Catalog profile matching the incoming SUBSCRIBE track name to the source encoder and sends with the selected bitrate/profile
- **Per-track encoders**: the publisher uses separate encoder workers for each camera/screen/audio Catalog track and sends with per-track settings such as codec and bitrate
- **Maintainability improvements**: Catalog subscription UI is rendered from role definitions (`video` / `screenshare` / `audio`), and Hook-side Catalog add/remove logic is shared per source type
- **Audio stream update modes**: each Audio track can use either `single stream` or `group update every N seconds`; the default is 1-second updates
- **Stable audio group rotation**: EndOfGroup is sent on group switches, and the WASM side closes/removes the relevant subgroup stream writer to avoid stream exhaustion
- **Per-user Stats modal**: a visualization button beside each remote user's Jitter settings opens a larger modal with bitrate, latency, keyframe interval, and related graphs
- **Video overlay cleanup**: bitrate and related stats text were removed from the video tag and consolidated into per-user Stats modals
- **Rendering rate calculation**: fps is calculated from intervals between decoder rendering events instead of the inverse of rendering latency
- **Latency visualization**: Stats include current values and time-series graphs for receive latency and playback latency
- **Unified latency timestamps**: the custom chunk `sentAt` value was removed; LoC `captureTimestamp` based on `performance.now()` just before encode is used for receive and buffer latency measurements
- **Latency drift correction**: audio/video encoders associate capture timestamps with input chunk timestamps rather than using FIFO queues, preventing monotonically increasing latency errors during long runs
- **Audio playout queue visualization**: subscriber-side audio queue time before `MediaStreamTrackGenerator.write` is shown in the `Audio Playout Queue` graph
- **Audio playout queue scale**: the `Audio Playout Queue` graph uses 10 ms tick intervals
- **Audio reset on resubscribe**: remote audio streams are explicitly closed on `UNSUBSCRIBE`, and audio elements are reinitialized on `SUBSCRIBE` to avoid carrying over playback state
- **Bitrate scale improvements**: Video/ScreenShare bitrate graphs use 250 kbps ticks and auto-adjust the upper bound from actual data instead of using a fixed 2000 kbps limit
- **Audio bitrate scale optimization**: Audio bitrate graphs show fixed tick lines at 30 / 60 / 90 / 120 / 160 / 200 kbps
- **FPS and latency scales**: Video frame rate graphs use 10 fps ticks, and latency graphs use 100 ms tick lines
- **Stats label cleanup**: the `Audio Rendering Rate (fps)` graph was removed, and `Rendering Latency (ms)` was renamed to `E2E Latency (ms)`
- **Latency label update**: `Receive Latency` was renamed to `Network Latency`, and `(ms)` was removed from latency titles
- **Latency graph consolidation**: Video and Audio each combine `Network` and `E2E` into one graph with legend labels and translucent filled areas
- **Keyframe interval visualization**: Stats include `Video / ScreenShare Keyframe Interval (frames)` graphs measured from received Chunk `type` (`key`)
- **Adaptive keyframe tick intervals**: Keyframe Interval graph tick spacing increases with the value range to avoid excessive ticks
- **Catalog extension cleanup**: `keyframeInterval`, `audioStreamUpdateMode`, and `audioStreamUpdateIntervalSeconds` are treated as local sender encoder-control settings and are not included in the Catalog
- **Stats line width adjustment**: Stats graph lines are thinner for better readability when overlapping
- **Scrollable chat sidebar**: the full Chat sidebar can scroll vertically so long conversations remain usable
- **Simplified Stats display**: instantaneous value cards and latest-value text were removed, consolidating Stats into graph views
- **Subscription button layout improvements**: track selection and `Subscribe/Unsubscribe` switch between horizontal and vertical layouts based on width to prevent overflow
- **Stats graph readability**: graph height was increased and Y-axis top/bottom padding is adjusted per metric
- **Stats tied to subscriptions**: Stats only show metrics with real data from active subscriptions; unsubscribed metrics and members without data are hidden
- **JitterBuffer block visualization**: each remote video shows Video/Audio buffer occupancy below the media area, rendered as one block per frame with up to 30 blocks and real-time push/pop updates
- **Right-aligned JitterBuffer blocks**: buffer blocks are colored from the right side to represent data accumulating near the tail
- **Video/Audio identification icons**: the JitterBuffer visualization row shows Video / Audio icons on the left side
- **Visualization toggle**: each remote receiver component has a grid icon button to toggle receiver-side JitterBuffer visualization independently
- **Simplified local controls**: Camera, Screen Share, and Mic controls use icon-only buttons

## MoQT Protocol Flow

### Track Namespace Structure

- **Track Namespace**: `/{RoomName}/{UserName}`
- **Catalog Track Name**: `catalog`
- **Camera Track Names**: `camera_1080p`, `camera_720p`, `camera_480p`
- **Screenshare Track Names**: `screenshare_1080p`, `screenshare_720p`, `screenshare_480p`
- **Audio Track Names**: `audio_128kbps`, `audio_64kbps`, `audio_32kbps`

### Message Sequence

1. **Connection and setup**

   - Establish the connection with the relay using `CLIENT_SETUP` / `SERVER_SETUP`

2. **Joining a room**

   - Subscribe to the `/{RoomName}/` prefix with `SUBSCRIBE_NAMESPACE`
   - Receive `PUBLISH_NAMESPACE` messages for existing and newly joined participants

3. **Media publishing**

   - The Catalog track is empty initially
   - Enabling camera adds three camera profiles, enabling mic adds three audio profiles, and enabling screen share adds three screen share profiles to the Catalog
   - Tracks for disabled media are not automatically removed from the Catalog because the disabled state may represent mute
   - Tracks can still be manually removed from the Catalog Tracks screen while the source is enabled, and that setting is preserved
   - Notify `/{RoomName}/{UserName}` with `PUBLISH_NAMESPACE`
   - When a Catalog `SUBSCRIBE` is received, send the `catalog` object on the `trackAlias` returned by `SUBSCRIBE_OK`
   - When the Catalog is updated, resend the Catalog object to notify track additions and deletions
   - Audio has an update mode per track; `every N seconds` tracks advance the groupId at the configured interval and publish on a new subgroup stream
   - Receive `SUBSCRIBE` messages from other participants
   - Start publishing on the `trackAlias` returned by `SUBSCRIBE_OK`
   - Send media data with `OBJECT` messages

4. **Media receiving**

   - Joining a room does not automatically subscribe to media
   - Run `Catalog Subscribe` from a participant card to get the track list from `catalog`
   - Continue receiving Catalog add/delete updates and reflect them in the UI
   - Select video/audio tracks and subscribe individually with `Subscribe Video` / `Subscribe Audio`
   - Receive media data with `OBJECT` messages

## Setup

### Install Dependencies

```bash
cd ../../
npm install
```

### Start the Development Server

```bash
npm run dev
```

Open http://localhost:5173/examples/call/ in a browser.

### Build

```bash
npm run build
```

## Project Structure

```
src/
├── components/
│   ├── ui/               # shadcn/ui components
│   │   ├── button.tsx
│   │   ├── card.tsx
│   │   ├── input.tsx
│   │   └── label.tsx
│   ├── JoinRoomForm.tsx      # room join form
│   ├── CallRoom.tsx          # main call room screen
│   ├── ParticipantCard.tsx   # participant card
│   └── PublishMediaPanel.tsx # media publishing panel
├── hooks/
│   └── useLocalSession.ts    # MoQT session management hook
├── types/
│   └── moqt.ts               # type definitions
├── lib/
│   └── utils.ts              # utility functions
├── App.tsx                   # main application
└── main.tsx                  # entry point
```

## TODO: WASM Integration

The MoQT client implementation is currently stubbed in `src/hooks/useLocalSession.ts`.
To integrate the actual WASM module (`moqt-client-wasm`), do the following:

1. Build the WASM module

   ```bash
   cd ../../../../bindings/wasm
   wasm-pack build --target web --features web_sys_unstable_apis
   ```

2. Enable the commented-out code in `useLocalSession.ts`

   - Import `import init, { MOQTClient }`
   - Call `init()`
   - Create a `MOQTClient` instance

3. Implement encoder/decoder workers
   - Use the existing `media/publisher` and `media/subscriber` code as references

## Technology Stack

- **React 19**: UI framework
- **TypeScript**: type-safe development
- **Tailwind CSS**: styling
- **shadcn/ui**: UI component library
- **Vite**: build tool
- **MoQT (WASM)**: Media over QUIC Transport protocol implementation

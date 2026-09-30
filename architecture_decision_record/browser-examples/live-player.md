# Live Player Architecture

## Status
Target design, adopted step by step (see "Migration"). Update this file in the
same change whenever the layering, module boundaries, public API or invariants
described here change.

## Scope
The Live Player is the MoQT viewer of `examples/browser`: it follows an MSF
catalog, subscribes to a video and an audio track, plays them live and replays
what the relay still caches through FETCH. It is extracted from the Live Viewer
example (`examples/live-viewer`), which keeps every feature it has today, and
is adopted by the ONVIF example (`examples/onvif`) as its second consumer.

Features carried over unchanged:

- MSF catalog following: SUBSCRIBE plus a FETCH of the group SUBSCRIBE_OK names
  as the largest, the catalog of the newest group wins, and a track redefined
  under the same name reconfigures its decoder.
- Track and packaging selection: video renditions, audio tracks, LOC or the
  `_cmaf` CMAF siblings.
- Live LOC playback: decoder workers, the pre-decode hold in the video worker,
  one playout clock with the adaptive jitter buffer and catch-up
  (`LivePlayout`), and a picture sink that is a `MediaStreamTrackGenerator` or
  a canvas.
- Live CMAF playback: `MseSink` on a pooled video element, waiting for a group
  start after every (re)open.
- Picture handover: one picture on screen at a time, a replacement is shown
  only once it has presented a frame.
- Seek axis: `GroupTimeline` built from observed capture timestamps within the
  relay cache TTL, and `MediaTimeline` from the MSF media timeline track.
- Review (DVR): seek and skip to a closed keyframe group, bounded FETCH
  windows fetched one ahead, live SUBSCRIBEs paused with Forward 0 and the live
  edge followed with TRACK_STATUS, recovery from evicted ranges, LOC review
  through `ReviewPlayout` and CMAF review through its own `MseSink`, playback
  speed for CMAF review.
- Pause, volume, back to live, stall detection.

## Layering

```
L3  page            examples/live-viewer, examples/onvif
                    connection form, MP4 publisher / PTZ, diagnostics
                    (StreamMonitor, delivery grid, PlaybackCharts, stats line), log
                        | public API and callbacks only
L2  controls        lib/player/ui
                    seek bar, skip buttons, play/pause, LIVE, volume, speed,
                    quality menu, fullscreen, pointer idle, buffering spinner
                        | public API and callbacks only
L1  LivePlayer      lib/player
                    catalog, subscriptions, live pipelines, seek timeline, review
                        |
L0  transport/codec @moqt/moqtClient, utils/media (decoder workers, MseSink)
```

Rules:

- L1 touches the DOM only through the picture elements it creates at the
  front of the container it is given (live video, live canvas, review canvas,
  the MSE pool), styled by `livePlayer.css`. It never reads form values or
  writes status text; overlays the page puts in the same container stay on
  top of the pictures.
- L1 does not connect. It is given a connected `MoqtClientWrapper`, so a page
  can share one session with a publisher or with other players.
- L1 does not register session-wide handlers (`setOnSubgroupHeaderHandler`,
  `setOnConnectionClosedHandler`); they have one slot per client and belong to
  the page. It registers per-alias and per-request handlers only.
- L2 holds no playback state of its own; it renders `LivePlayer.state` and
  issues commands.
- L1 still reads the MSF catalog through `examples/media/catalog.ts` and uses
  `getErrorMessage` from `examples/media/common.ts`; the catalog reader shares
  its audio track naming with the media publishers there.
- Diagnostics stay in L3. L1 reports deliveries to an observer whose shape is
  the subset of `StreamMonitor` it calls (`DeliveryObserver`).

## Public API (L1)

```ts
type LivePlayerOptions = {
  client: MoqtClientWrapper
  authInfo: string
  container: HTMLElement
  callbacks: LivePlayerCallbacks
  deliveryObserver: DeliveryObserver
  livePicture?: LivePictureKind
}

type LivePlayerCallbacks = {
  onStateChange(): void
  onLiveFrame(): void
  onLog(level: 'info' | 'warn' | 'error', message: string): void
}

class LivePlayer {
  start(namespace: string[]): Promise<void>
  stop(): Promise<void>
  selectVideoTrack(name: string): Promise<void>
  selectAudioTrack(name: string): Promise<void>
  setPackaging(packaging: Packaging): Promise<void>
  seek(captureMicros: number): void
  skip(seconds: number): void
  goLive(): void
  setPaused(paused: boolean): void
  setVolume(volume: number): void
  setPlaybackRate(rate: number): void
  setBufferPolicy(policy: BufferPolicy): void
  setCatchUp(catchUp: CatchUp): void
  readonly state: LivePlayerState
  stats(): LivePlayerStats
  elapsedMsAt(captureMicros: number): number | undefined
}
```

One player lives as long as the page; `start` and `stop` bracket one watch,
and the settings (packaging, volume, buffer policy, catch-up) carry over.

`LivePlayerState` carries what the controls and the diagnostics render: the
catalog tracks and the selection, whether CMAF is available, the packaging,
`mode: 'live' | 'review'`, paused, stalled, the playback rate and whether it
is adjustable,
the catalog / playback / rewind status, the seek range (broadcast start,
replayable start, live edge, review anchor and playhead, and elapsed times from
the media timeline), and the aliases of the live subscriptions and review
FETCHes the delivery grid follows. `LivePlayerStats` carries the numbers of the
stats line and the Buffer chart: viewer delay, buffer and target, output
latency, arrival spread, bitrate, A/V offset, audio breaks, video drops, shed.

## Modules (L1)

| Module | Responsibility |
| --- | --- |
| `livePlayer` | Facade: owns the modules below and the state, dispatches commands, decides by packaging |
| `trackContext` | The client, namespace, auth info, observer and log every request needs |
| `deliveryObserver` | What the player reports of every object, FETCH and playhead |
| `catalogFollower` / `textTrack` | Catalog SUBSCRIBE + FETCH, newest group wins; text tracks such as the media timeline |
| `trackSubscriptions` | SUBSCRIBE / UNSUBSCRIBE per media kind, Forward updates chained in order |
| `locLive` | Live LOC: decoder workers, `LivePlayout`, picture sink, the worker's pre-decode hold |
| `cmafLive` | Live CMAF: the live `MseSink`, waiting for a group start after every (re)open |
| `pictureStage` | One picture on screen, handover to a picture once it has presented a frame, the MSE element pool |
| `seekTimeline` | `GroupTimeline` + `MediaTimeline`, stamping groups from the media timeline, the seek axis |
| `reviewSession` | One instance per seek: FETCH windows, waiting for closed groups, following the live edge with TRACK_STATUS, eviction recovery; `isCurrent` replaces generation counters |
| `reviewFetch` | One bounded FETCH of a window, its stream end and failure codes |
| `locReview` / `cmafReview` | Playing a fetched window through `ReviewPlayout` or a review `MseSink` |
| `stallWatch` | Whether the wanted picture has stood still |
| `streamConventions` | The relay cache TTL the seek axis keeps groups for |

The existing classes (`LivePlayout`, `PlayoutClock`, `JitterBuffer`,
`AudioPlayout`, `AudioSplice`, `VideoPlayout`, `LivePictureSink`,
`ReviewPlayout`, `GroupTimeline`, `MediaTimeline`) move as they are.

Each player creates its own decoder workers, two per instance.

## Stream conventions

The player relies on how the publishers shape their tracks. They are written
where they are used, not as options, until a publisher differs:

| Convention | live-ingest / MP4 publisher | ONVIF bridge |
| --- | --- | --- |
| LOC capture timestamps on one wall clock for all tracks | yes | yes |
| CMAF sibling named `<track>_cmaf` | live-ingest only | none |
| MSF media timeline track | yes | none (the seek axis falls back to the replayable window) |
| Audio groups start at the video keyframes with the same ids | yes | **no**: audio rotates every 2 s with its own ids |
| Codec in the catalog from the first catalog | yes | only once the track's stream has started; a catalog update fills it in |
| Relay cache TTL 60 s, join group holds no keyframe | yes | yes |

The audio row is the one difference that changes behavior: review fetches the
audio of a window by the video's group range. For ONVIF the audio range has to
be resolved from the capture times of observed audio groups instead, which is
the generalization the ONVIF adoption brings.

## Invariants

- Only one review runs at a time; a new seek, `goLive`, a video track change
  or `stop` ends the current one, and nothing an ended review started may touch
  the picture, the playouts or the state afterwards.
- A review FETCH never reaches into the open live edge group.
- Forward updates reach every live subscription in the order they were issued.
- A picture that has not presented a frame is never shown.
- The pre-decode hold stays in the video worker and the playout budget on the
  main thread (see `LivePlayout`); the worker's superseded-group drop stays.

## Migration

Each step is its own PR, stacked on the previous one; the Live Viewer E2E
suites (`e2e:live-viewer`, `e2e:live-viewer-mp4`, `e2e:live-viewer-delivery`)
must pass through steps 1 to 3. The test ids of the page keep their
`live-viewer-*` names; the picture elements the player creates are
`live-player-video`, `live-player-live-canvas` and `live-player-review-canvas`.

1. Split `live-viewer/main.ts` into the L1 modules inside the example, with
   `main.ts` reduced to page wiring. No behavior change.
2. Move L1 to `lib/player`; the player creates its picture elements in the
   container; split `viewer.css` into player and page styles.
3. Extract the controls into `lib/player/ui`.
4. Resolve review audio by capture time (the ONVIF convention above).
5. Adopt the player in `examples/onvif`, keeping PTZ and its diagnostics in
   the page.

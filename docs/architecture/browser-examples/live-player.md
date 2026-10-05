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
- L2 (`PlayerControls`) holds no playback state of its own; it renders
  `LivePlayer.state` when the page forwards `onStateChange`, issues commands,
  and keeps only the gesture in progress (a seek bar drag, pointer idle). It
  appends its markup and `playerControls.css` to the same container, after the
  pictures and the page's own overlays. The arrow keys skip wherever the focus
  is, so a page holds one `PlayerControls`.
- L1 still reads the MSF catalog through `examples/media/catalog.ts` and uses
  `getErrorMessage` from `examples/media/common.ts`; the catalog reader shares
  its audio track naming with the media publishers there.
- Diagnostics stay in L3. L1 reports deliveries to an optional observer
  (`DeliveryObserver`), which the Live Viewer's `StreamMonitor` implements.

## Public API (L1)

```ts
type LivePlayerOptions = {
  client: MoqtClientWrapper
  container: HTMLElement
  callbacks: LivePlayerCallbacks
  deliveryObserver?: DeliveryObserver
  livePicture?: LivePictureKind
}

type LivePlayerCallbacks = {
  onStateChange(): void
  onLiveFrame(): void
  onLog(level: 'info' | 'warn' | 'error', message: string): void
}

class LivePlayer {
  start(namespace: string[], authInfo: string): Promise<void>
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

`LocLive` is the live LOC pipeline on its own, for a page that keeps its own
catalog and subscriptions and only needs the playback: it is given the picture
elements (`createPictureVideo` / `createPictureCanvas` from
`pictureElements.ts`), is told the catalog track of each kind, and is fed the
subgroup objects the page receives.

```ts
class LocLive {
  constructor(video: HTMLVideoElement, canvas: HTMLCanvasElement, livePicture: LivePictureKind | undefined, callbacks: LocLiveCallbacks)
  readonly playout: LivePlayout
  readonly picture: LivePictureSink
  configureTrack(kind: MediaKind, track: MediaCatalogTrack): void
  push(kind: MediaKind, groupId: bigint, object: SubgroupObjectMessageWithLoc): void
  setBufferPolicy(policy: BufferPolicy): void
  reset(): void
  dispose(): void
  stats(): LiveStats
}
```

`reset` empties the playout and detaches the picture; `dispose` also ends the
decoder workers and closes the `AudioContext`, so a page that creates one
`LocLive` per stream it shows can let it go. `LiveStats` is what
`LivePlayer.stats()` reports minus the object count: frame size, viewer delay,
buffer and target, output latency, arrival spread, video and audio bitrate,
A/V offset, audio breaks, video drops, shed.

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
| `locLive` | Live LOC: decoder workers, `LivePlayout`, picture sink, the worker's pre-decode hold; its stats and its disposal, so pages other than `LivePlayer` can hold one per stream |
| `pictureElements` | The hidden, autoplaying `<video>` / `<canvas>` a live picture is shown in, shared by `PictureStage` and the pages that create their own |
| `cmafLive` | Live CMAF: the live `MseSink`, waiting for a group start after every (re)open |
| `pictureStage` | One picture on screen, handover to a picture once it has presented a frame, the MSE element pool |
| `seekTimeline` | `GroupTimeline` + `MediaTimeline` + `AudioGroups`, stamping groups from the media timeline, the seek axis |
| `audioGroups` | Where the audio groups start, whether they share the video group ids, which of them cover a window |
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

The audio row is the one difference that changes behavior. `AudioGroups`
records where each audio group starts on the capture axis, from live audio
objects and, for groups known only from TRACK_STATUS during a review, from the
arrival lag of the last live object. When every group seen on both tracks
starts within 500 ms of the video group with the same id, review fetches a
window's audio by the video's group range, as before; otherwise it fetches the
closed audio groups whose starts cover the window's capture span, waiting until
an audio group has started past the window. Audio chunks an earlier window
already played are left out, since a 2 s audio group can span two windows.

## Invariants

- Only one review runs at a time; a new seek, `goLive`, a video track change
  or `stop` ends the current one, and nothing an ended review started may touch
  the picture, the playouts or the state afterwards.
- A review FETCH never reaches into the open live edge group.
- Forward updates reach every live subscription in the order they were issued.
- A picture that has not presented a frame is never shown.
- The pre-decode hold stays in the video worker and the playout budget on the
  main thread (see `LivePlayout`); the worker's superseded-group drop stays.

## Playback behaviour

What the Live Viewer page documents to its users as controls; this section
records how they work.

### Review (rewind)

The skip buttons, the arrow keys and the seek bar all FETCH groups the relay
still caches and play them paced by their capture timestamps: video on the
review canvas, audio on a review `AudioContext`. `LIVE` returns to live.

- A position is derived from the capture timestamps observed during live
  playback. The publishers number groups from the wall clock and switch groups
  at encoder keyframes, so a difference of group ids is not a number of seconds.
- A review FETCHes from the closed keyframe group that contains the target,
  decodes the frames before the target without pacing and draws from the target
  on; in MSE the same amount is added to `currentTime` before playing.
- The range is limited to groups the publisher has closed. A range that reaches
  into an open group leaves the relay cache, is forwarded upstream and answered
  from the publisher's 60 s cache.
- The relay keeps objects for 60 s by default (`RELAY_CACHE_TTL_SECS`) and the
  publishers for 60 s; nothing older can be reached.
- While reviewing, the video and audio SUBSCRIBEs are set to Forward 0 with
  SUBSCRIBE_UPDATE so no live objects arrive. Groups the publisher opens in the
  meantime are learned from the Largest Location of a TRACK_STATUS every
  500 ms, which advances the next FETCH range and the right end of the seek bar.
  Their positions come from the encode time the media timeline records; for a
  rendition the timeline does not cover they are estimated from the arrival
  lag of the last live object and drift ahead while the main thread is busy.
  `LIVE` restores Forward 1; the relay resumes from the next group it opens,
  so the live picture takes up to one GOP to move. Against a relay that does
  not answer TRACK_STATUS the subscriptions stay on Forward 1 during review.

### MP4 publisher

The MP4 Publish panel demuxes the chosen file in the browser and publishes it
as the catalog and LOC tracks `live-ingest` produces, plus the media timeline,
so the viewer and the rewind work unchanged. No CMAF siblings are published.

- Demuxing uses the progressive MP4 index of `crates/mediapack`
  (`mp4::Mp4Index`) through `crates/moqt-client-wasm`: only `moov` is handed to wasm and
  samples are read with `File.slice`, so the file never sits in memory whole.
- Video must be H.264. `mediapack` converts AVCC samples to Annex-B and puts
  the SPS / PPS from `avcC` before every keyframe, so the catalog carries no
  `initData`, as with `live-ingest`.
- Audio may be AAC or MP3, both sent as the MP4 frames they are (a LOC payload
  is the raw bitstream of a codec in the WebCodecs registry). AAC puts its
  AudioSpecificConfig in the catalog `initData`; MP3 needs only
  `codec: "mp3"` for the receiver's `AudioDecoder`. Other audio codecs are
  skipped and the video is published alone.
- It does not wait for a SUBSCRIBE: `video`, `audio`, `timeline` and `catalog`
  are PUBLISHed, the catalog after every track it lists so the relay caches
  each from its first object, and the catalog is re-sent every 30 s so it never
  leaves the relay cache. A SUBSCRIBE that reaches the publisher is rejected
  with NOT_SUPPORTED.
- Groups switch at video keyframes and audio objects join the group of the
  preceding keyframe. Group ids, the catalog's included, are numbered from the
  start time in unix microseconds, so a republished file never reuses a
  location (the relay keeps a track's cache across publishers and treats a
  reused location as a malformed track).
- The `timeline` track sends, at every keyframe, the records so far
  (presentation time, `[group id, 0]`, encode wallclock) as one object in a new
  group; records older than the relay's 60 s retention are dropped and
  presentation time counts from the start of publishing.
- Samples are sent in decode order, like a live encoder with B-frames, at a
  wall clock of decode time plus the file's reorder delay (the largest amount
  presentation time runs ahead of decode time; 0 without B-frames), and the
  presentation wall clock travels as the LOC capture timestamp. Loop shifts the
  next pass by the file's duration.
- The preview decodes the samples as sent with WebCodecs and draws each frame
  at capture timestamp plus reorder delay, the moment everything up to that
  frame has been sent, giving the picture of a receiver without network or
  buffer delay. The viewer's LOC frames carry the same capture timestamps, so
  the difference to their display time is shown as `viewer delay` under the
  preview and as `delay` in the Playback stats; across two browsers the clock
  skew between them adds to it.
- Publish Streams shows the groups (subgroup streams) sent per track as the
  same bars as Subscribe Streams under the Playback picture, sharing its Window
  and GOPs settings, with the counts of streams in flight and finished and the
  send bitrate of the streams open in the window.
- Publishing runs on its own MoQT session, so Watch and Stop on the same page
  do not interrupt it. Browsers do not answer FETCH, so rewinding is limited to
  the closed groups in the relay cache.

### Packaging

The gear menu selects how the media tracks are received. `LOC` subscribes to
the `loc` tracks and decodes them with WebCodecs into a MediaStream; `CMAF`
subscribes to the `_cmaf` siblings the bridge publishes alongside them
(draft-ietf-moq-cmsf-01) and feeds their fragments to a MediaSource on the
same video element, with the init segment taken from the catalog `initData`.
Switching packaging re-subscribes both tracks at the current quality.

In CMAF mode the seek bar and rewind targets are built from the MSF media
timeline, because CMAF objects carry no LOC capture timestamp, and review
playback appends the fetched fragments to a MediaSource instead of drawing them
on the canvas. Every MediaSource — live, review, or the replacement opened by a
packaging or quality change — takes its own video element from a small pool, and
a new picture is shown only once it has presented a frame while the previous one
stays on screen until then. The live MediaSource stays open hidden behind a
review but receives nothing while the live subscriptions do not forward, so
after `LIVE` the fragments of the next group land in a buffered range of their
own and playback jumps there once it holds a second of video. The live audio
still buffered when a review starts is silenced and heard again on `LIVE`.

### Review audio

The bridge starts the audio groups at the video keyframes with the same ids,
so the audio of a window is the same group range on the audio track and is
fetched alongside the video. The audio of a group ends a little after its
video, because the source interleaves audio behind video, so an audio group is
fetched once the audio track has moved on to a later group; fetched earlier,
its tail would be missing and MSE would stall on the hole. In LOC mode the chunks are decoded up front and
scheduled on the review's own clock, which maps capture timestamps onto local
time from the position the review starts at and follows the drift the audio
device shows, so the picture keeps step with the sound; frames are decoded a
little ahead of their presentation rather than a whole window at once. In CMAF
mode the fragments are appended to the review MediaSource, which aligns them
by `tfdt`. The rewind status shows the review's own `A/V` offset.

### Catalog

The catalog is subscribed to for updates and fetched for its current object:
a SUBSCRIBE delivers objects published after the largest one, and the
publishers send the catalog when it changes and every 30 seconds, so a viewer
joining in between would otherwise wait for the next one. The FETCH names the
group SUBSCRIBE_OK reports as the largest, which the publishers republish
before the relay cache drops it. When SUBSCRIBE_OK says no content exists yet, nothing is fetched and
the catalog arrives on the SUBSCRIBE; a FETCH the relay cannot cover would be
forwarded to the publisher, and the MP4 publisher does not answer it.

The two may deliver different catalogs: the relay keeps the catalog of a
publisher that has since been replaced, so the FETCH can return the old one
while the SUBSCRIBE brings the new one. The viewer applies the catalog of the
newest group whatever order they arrive in, and when a catalog redefines a
track it is subscribed to under the same name, as a new publisher with another
audio codec does, the decoder is reconfigured from the new definition.

### Audio / video synchronisation

In LOC mode the two decoder workers hand every sample over as soon as it is
decoded, and one playout clock decides when each is presented. The clock maps
the LOC capture timestamps, which the bridge stamps on the same wall clock for
every track, onto the local clock behind a jitter buffer. The buffer tracks
the delay from capture to arrival of the audio chunks of the last 10 s and
covers their peak-to-peak jitter (the slowest minus the fastest) plus the audio
device's output latency (the time from handing a chunk to the device to
hearing it, some 25 ms on built-in speakers and far more over Bluetooth). That
sum is clamped to `Min buffer (ms)` (200 ms by default) and `Max buffer (ms)`
(unlimited when empty), so the bounds are the buffer as displayed; equal
bounds fix it, and a maximum below the output latency makes every chunk late.
`Current` next to the bounds shows the buffer and its target, and the Buffer
chart below the player stacks the output latency and `audio jitter (p-p)` under
the target and the buffer held, with delay, bitrate and A/V charted beside it
over the last minute. A subscription opens
with a burst of what the relay had cached of the current groups, so the
samples of the first 400 ms are held and the clock is anchored on the newest
of them; older ones are dropped rather than played late. That burst says
nothing about the jitter, so the buffer opens on the longest wait
between two audio arrivals seen during the warm-up instead: sources such as
MPEG-TS over SRT deliver audio in bursts. The audio is the clock's master: it
alone moves the clock, so the sound never skips for the picture, and the
picture takes over only while no audio is playing. Video frames are held until
they are due and then written to the MediaStream the video element shows;
every move of the clock applies to the frames captured from the sample that
caused it on, so the waiting frames move with the sound they belong to, and no
frame is due before one captured earlier. Audio is scheduled on an
`AudioContext` running at the stream's sample rate. Chunks are appended whole
at a write head so the waveform stays continuous: capture timestamps are
millisecond-precise and the context clock is read a render quantum at a time,
so a chunk placed on its own target would leave a gap or an overlap each time.
The distance between the write head and the target is fed back to the clock,
which makes the audio device the master the picture follows; a late chunk is
not trimmed but starts at once and moves the clock the same way, so the chunks
behind it stay contiguous; that is how the buffer grows. Once the buffer has
settled (10 s unless its bounds are equal), `Catch up` decides how a buffer above its
target shrinks. `Skip (trim + crossfade)`, the default, drops each audio chunk that fits in the
excess and fades the next one in from the head of the first dropped chunk, at
the offset within 10 ms where the two waveforms look most alike, so the sound
jumps without a click. `Speed up (WSOLA)` shortens the audio by 10 % while the buffer
is more than 20 ms over, without changing its pitch: where the waveform
repeats, one period of 2.5–10 ms is overlapped onto the next with a 5 ms
crossfade (the time-scale modification WebRTC's NetEq calls accelerate), and
it skips like `Skip (trim + crossfade)` once the buffer is more than 400 ms over. `Off` never
shrinks it. The stats line shows the buffer and its target as
`buffer N ms (target M)` and how much latency the catch-up has taken out as
`shed N ms`.
The stats line shows the offset between the picture on screen and the sound as
`A/V +N ms`, as `audio breaks N` how often the sound did not continue where
the previous chunk ended, and as `video N dropped / M late` how many frames
fell due together with a newer one or arrived after a newer one was shown and
were never shown, and how many were shown more than a frame period after they
were due.

The audio decoder stamps its outputs from the sample count it has produced,
not from the timestamps of the chunks, so a hole in the source, such as a lost
frame or a file that loops, would shift every later output and leave the sound
ahead of the picture for as long as the decoder lives. Each output is
therefore labelled with the capture timestamp of the chunk it was decoded
from, in the live and the review decoder alike. The relay delivers each group
on its own stream, and when the tail of one audio group and the head of the
next are in flight together the streams interleave and the head lands first;
the audio worker holds the objects of a later group until the group before
them has ended, for at most 100 ms, so the decoder sees them in order. The
video worker instead moves on with the keyframe of the newer group as soon as
it arrives and drops what is left of the older group: those frames predict
from references the keyframe has replaced, and decoding them would smear the
picture until the next keyframe and show a frame older than the one on screen.

In CMAF mode the MediaSource does the same from the `tfdt` of the fragments,
which the bridge writes on one timeline for both tracks, so the SourceBuffers
append in the default segments mode rather than back to back.

### Playback speed

The speed control next to the rewind buttons offers 0.5x, 1x, 1.25x, 1.5x and
2x. It is enabled only while reviewing in CMAF mode, where the fetched fragments
play through a MediaSource and the element's `playbackRate` applies; live
playback has to keep pace with the publisher and the WebCodecs path paces
frames itself. Returning to live resets the rate to 1x, and review that runs
faster than real time returns to live by itself once it has caught up with the
live edge, instead of stalling on every group the publisher has yet to close.
Audio, once review carries it, is time-stretched by the browser (`preservesPitch`
is on by default).

### Player controls

The seek bar, the skip buttons and the quality menu sit on the video itself
rather than in their own cards. They appear while the pointer is over the
picture, while a control has focus or while the quality menu is open, and fade
out otherwise. The gear opens the video and audio track selection and the
packaging choice, and closes on a second click, on Escape, or on a click
outside it.

A spinner appears in the centre of the picture once it has stood still for half
a second while playback is meant to be running: the data is arriving too slowly
to decode the next group, or the window a seek asked for is still being fetched.
It disappears with the next frame, and is not shown while paused or stopped.

The centre button pauses and resumes whatever is on screen. Every other
transition — seek, skip, `LIVE`, a packaging or quality change — resumes.
Pausing a review freezes the picture and the sound where they are and resuming
carries on from there; pausing live playback stops both, and resuming returns
to the live edge (live CMAF jumps to the end of what is buffered, live LOC
warms up again). The volume slider next to the speed control drives the live audio output
(the `AudioContext` gain for LOC, the MediaSource element for CMAF).

The `LIVE` button returns to the live edge. It is translucent with a red dot
while playback is live and filled while playback is behind the live edge, so the
button doubles as the indicator for which of the two the viewer is watching.

The button at the right end of the controls puts the picture into fullscreen and
takes it out again (Escape also leaves). In fullscreen the pointer never leaves
the picture, so the controls and the cursor hide once the pointer has rested for
a few seconds and come back when it moves.

### Seek bar

The slider below the video spans the whole broadcast: its left end is the start,
placed by the MSF media timeline, and its right end is the live edge. The thin bar
underneath marks the part of that range the relay still caches and can therefore
replay, which is the only part a seek resolves. The relay cannot be asked which
groups it still holds, so the bar covers the groups the viewer observed within the
relay's default cache TTL (60 s) and shrinks as they age out. Home jumps to the oldest
replayable position rather than to the start of the broadcast, and End returns to
live.

While review playback runs, the thumb stays on the position that was seeked to
and a fill from it carries the movement, because the decoder emits frames in
bursts and a thumb that followed each one read as jitter. The fill and the
readouts step a second at a time for the same reason.

The position label shows seconds behind live and follows review playback. A seek
decodes from the preceding closed keyframe group, shows frames from the chosen
position on, and continues with the same bounded FETCH replay as the skip
buttons.

Review playback does not stop at the end of the fetched window: the next
bounded FETCH is issued while the current window plays, so playback keeps
running behind the live edge until Live is pressed. Because the window is paced
by capture timestamps it never catches up on its own, and when it reaches the
newest closed group it waits for the publisher to close another one.

The slider is disabled until a closed group is available. Stop and video quality
changes clear the timeline and cancel pending review playback. The observed
range is not a guarantee of relay cache retention: an evicted group can no longer
be replayed. A FETCH that fails with INTERNAL_ERROR, TIMEOUT, INVALID_RANGE,
NO_OBJECTS or UNKNOWN_STATUS_IN_RANGE, whose stream is reset with INTERNAL_ERROR,
or that returns no objects, is taken as eviction: the groups up to the failed one leave the
timeline and review resumes from the oldest group left, or goes live when none
is. Any other failure reports the FETCH error and holds the requested position,
so pick another position or press Live to resume.

### Broadcast elapsed time

The readout above the slider shows `position / broadcast`, both measured from the
start of the broadcast, and the label at its left end the elapsed time at the axis
start. The numbers come from the MSF media timeline track
(draft-ietf-moq-msf-01 section 7), which the bridge publishes as a JSON array of
`[presentation time, [group id, object id], encode wallclock]` records covering
the groups the relay still caches. The viewer finds it in the catalog by its
`mediatimeline` packaging and subscribes to it alongside the media tracks.

A capture timestamp resolves to a presentation time by offsetting from the
newest record at or before it, so the reading survives a video quality change
even though renditions number their groups independently. The label shows
`--:-- / --:--` until the first timeline object arrives.

## Migration

Each step is its own PR, stacked on the previous one; the Live Viewer E2E
suites (`e2e:live-viewer`, `e2e:live-viewer-mp4`, `e2e:live-viewer-delivery`)
must pass through steps 1 to 3. The test ids of the page keep their
`live-viewer-*` names; the elements the player and its controls create are
named `live-player-*`.

1. Split `live-viewer/main.ts` into the L1 modules inside the example, with
   `main.ts` reduced to page wiring. No behavior change.
2. Move L1 to `lib/player`; the player creates its picture elements in the
   container; split `viewer.css` into player and page styles.
3. Extract the controls into `lib/player/ui`.
4. Resolve review audio by capture time (the ONVIF convention above).
5. Adopt the player in `examples/onvif`, keeping the PTZ commands in the page.

## Consumers

- `examples/live-viewer`: the player, its controls, and the page's diagnostics
  (StreamMonitor, delivery grid, PlaybackCharts), the MP4 publisher and the
  connection form.
- `examples/onvif`: the player and its controls on the session that also
  announces the viewer namespace and answers the bridge's SUBSCRIBE for the
  PTZ command track. Latency follows the player's jitter buffer; the worker
  pacing presets, the jitter-buffer visualizer and the worker-telemetry stats
  the page used to show are gone with it, and the catalog, video and audio are
  subscribed by the player instead of by request ids typed into the page.
- `examples/meeting`: `LocLive` without `LivePlayer`, since the meeting has its
  own catalog and subscribes per participant. Each remote member plays through
  one `LocLive` for the camera and the audio and another for a screen share;
  the pipeline is replaced when its video track is unsubscribed, so the next
  video track starts on fresh decoders, and audio joining a running picture
  warms the playout up again. The buffer policy and catch-up are set per
  member, and the per-member stats line and charts show `LiveStats`. The
  worker pacing presets, the jitter-buffer visualizer and the worker-telemetry
  charts are gone with it, as in `examples/onvif`.
- `examples/media/subscriber`: one `LocLive` on the page's own `<video>` and a
  hidden `<canvas>`, configured from the MSF catalog tracks the page selects
  and fed from its SUBSCRIBEs; the playout stats line replaces the jitter
  buffer bypass checkbox.

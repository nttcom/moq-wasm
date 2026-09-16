# live-viewer

`bridges/live-ingest` が RTMP / SRT から MoQT へ流した配信を視聴します。catalog に載っている映像
track を切り替えられるので、`--transcode` で生成した下位画質（`video_480p` / `video_360p`）の確認にも
使えます。

## 使い方

```shell
make relay
make live-ingest            # LIVE_INGEST_TRANSCODE=1 で下位画質も配信する
                            # SRT を GStreamer 経由で流すなら make gst-srt-publish
make ffmpeg-rtmp            # または make ffmpeg-srt / make ffmpeg-srt-bbb
make browser
make chrome                 # 自己署名証明書の relay に接続するため
```

Chrome で `examples/live-viewer/index.html` を開き、relay URL と namespace（RTMP は `app/stream`、
SRT は stream ID）を入れて Watch を押します。`?moqtUrl=...&trackNamespace=...` でも指定できます。

## 巻き戻し

映像にカーソルを合わせると中央に `↺5` `↺1` `1↻` `5↻` が出ます。キーボードでは ← / → が 1 秒、
↓ / ↑ が 5 秒です。どれも relay のキャッシュに残っている group を FETCH で取り出し、映像は review canvas
に、音声は review 用の AudioContext に、capture timestamp のとおりのペースで揃えて再生します。`LIVE` で
ライブ表示へ戻ります。

- 位置は、ライブ再生中に観測した capture timestamp から求めます。bridge は group id を
  壁時計で採番し、group はエンコーダの keyframe ごとに切り替わるため、group id の差は秒数になりません。
- 目標位置を含む閉じた keyframe group から FETCH し、目標より前のフレームはペースをかけずにデコード
  だけして、目標以降のフレームから描画します。MSE では同じ分だけ `currentTime` を進めて再生を始めます。
- 取得範囲は publisher が書き込みを終えた group までに制限します。開いている group に伸ばすと
  relay のキャッシュを外れて上流へ転送され、publisher 側キャッシュ（30 秒）から返されます。
- relay のキャッシュ保持は既定 30 分（`RELAY_CACHE_TTL_SECS`）、publisher 側は 30 秒です。
  それより前へは戻れません。

## ペイロード形式

live-ingest もブラウザ publisher も object を LOC（draft-ietf-moq-loc-01）で送ります。payload は
コーデックのビットストリームそのもので、capture timestamp などの metadata は MoQT の LOC 拡張
ヘッダに載ります。音声の AudioSpecificConfig は catalog の `initData` から取ります。

## E2E

relay・live-ingest・配信元・dev server が動いている状態で実行します。

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=live \
npm --prefix examples/browser run e2e:live-viewer
```

## Packaging

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
stays on screen until then; the live picture keeps decoding hidden behind a
review, so `LIVE` swaps back at once. The live audio is silenced while
reviewing and heard again on `LIVE`.

## Review audio

The bridge starts the audio groups at the video keyframes with the same ids,
so the audio of a window is the same group range on the audio track and is
fetched alongside the video. In LOC mode the chunks are decoded up front and
scheduled on the review's own clock, which maps capture timestamps onto local
time from the position the review starts at and follows the drift the audio
device shows, so the picture keeps step with the sound; frames are decoded a
little ahead of their presentation rather than a whole window at once. In CMAF
mode the fragments are appended to the review MediaSource, which aligns them
by `tfdt`. The rewind status shows the review's own `A/V` offset.

## Catalog

The catalog is subscribed to for updates and fetched for its current object:
a SUBSCRIBE delivers objects published after the largest one, and the bridge
publishes the catalog once per upstream subscription, so a viewer joining a
subscription the relay already holds would otherwise never see it. The FETCH
names the group SUBSCRIBE_OK reports when the relay still knows it and the
whole track otherwise, which the relay completes from the bridge.

## Audio / video synchronisation

In LOC mode the two decoder workers hand every sample over as soon as it is
decoded, and one playout clock decides when each is presented. The clock maps
the LOC capture timestamps, which the bridge stamps on the same wall clock for
every track, onto the local clock with a 200 ms delay that is the jitter
budget. A subscription opens with a burst of what the relay had cached of the
current groups, so the samples of the first 400 ms are held and the clock is
anchored on the newest of them; older ones are dropped rather than played
late. Sources such as MPEG-TS over SRT deliver audio in bursts, so the longest
wait between two audio arrivals seen during that warm-up is added to the
budget. The audio is the clock's master: it alone moves the clock, so the sound
never skips for the picture, and the picture takes over only while no audio is
playing. Video frames are held until they
are due and then written to the MediaStream the video element shows; audio is
scheduled on an `AudioContext` running at the stream's sample rate. Chunks are
appended whole at a write head so the waveform stays continuous: capture
timestamps are millisecond-precise and the context clock is read a render
quantum at a time, so a chunk placed on its own target would leave a gap or an
overlap each time. The distance between the write head and the target is fed
back to the clock, which makes the audio device the master the picture
follows; a late chunk is not trimmed but starts at once and moves the clock
the same way, so the chunks behind it stay contiguous. An audio chunk due more
than 400 ms past the budget re-anchors the clock so the extra latency is shed.
The stats line shows the offset between the picture on screen and the sound as
`A/V +N ms` and, as `audio breaks N`, how often the sound did not continue
where the previous chunk ended.

In CMAF mode the MediaSource does the same from the `tfdt` of the fragments,
which the bridge writes on one timeline for both tracks, so the SourceBuffers
append in the default segments mode rather than back to back.

## Playback speed

The speed control next to the rewind buttons offers 0.5x, 1x, 1.25x, 1.5x and
2x. It is enabled only while reviewing in CMAF mode, where the fetched fragments
play through a MediaSource and the element's `playbackRate` applies; live
playback has to keep pace with the publisher and the WebCodecs path paces
frames itself. Returning to live resets the rate to 1x, and review that runs
faster than real time returns to live by itself once it has caught up with the
live edge, instead of stalling on every group the publisher has yet to close.
Audio, once review carries it, is time-stretched by the browser (`preservesPitch`
is on by default).

## Player controls

The seek bar, the skip buttons and the quality menu sit on the video itself
rather than in their own cards. The skip buttons appear while the pointer is over
the picture. The gear opens the video and audio track selection and the
packaging choice, and closes on a second click, on Escape, or on a click
outside it.

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

## Seek bar

The slider below the video spans the whole broadcast: its left end is the start,
placed by the MSF media timeline, and its right end is the live edge. The thin bar
underneath marks the part of that range the relay still caches and can therefore
replay, which is the only part a seek resolves. Home jumps to the oldest
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
be replayed. A failed seek reports the FETCH error and holds the requested
position, so pick another position or press Live to resume.

## Broadcast elapsed time

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

# live-viewer

`bridges/live-ingest` が RTMP / SRT から MoQT へ流した配信を視聴します。catalog に載っている映像
track を切り替えられるので、`--transcode` で生成した下位画質（`video_480p` / `video_360p`）の確認にも
使えます。

## 使い方

```shell
make relay
make live-ingest            # LIVE_INGEST_TRANSCODE=1 で下位画質も配信する
make ffmpeg-rtmp            # または make ffmpeg-srt
make browser
make chrome                 # 自己署名証明書の relay に接続するため
```

Chrome で `examples/live-viewer/index.html` を開き、relay URL と namespace（RTMP は `app/stream`、
SRT は stream ID）を入れて Watch を押します。`?moqtUrl=...&trackNamespace=...` でも指定できます。

## 巻き戻し

`-10s` / `-30s` は relay のキャッシュに残っている group を FETCH で取り出し、review canvas に
capture timestamp のとおりのペースで再生します。`Back to live` でライブ表示へ戻ります。

- 何秒戻るかは、ライブ再生中に観測した capture timestamp から求めます。bridge は group id を
  壁時計で採番し、group はエンコーダの keyframe ごとに切り替わるため、group id の差は秒数になりません。
- 取得範囲は publisher が書き込みを終えた group までに制限します。開いている group に伸ばすと
  relay のキャッシュを外れて上流へ転送され、FETCH を提供しない bridge が `NOT_SUPPORTED` を返します。
- relay のキャッシュ保持は既定 30 秒（`RELAY_CACHE_TTL_SECS`）です。それより前へは戻れません。

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
playback appends the fetched fragments to a fresh MediaSource instead of
drawing them on the canvas. Review plays video only in either mode.

## Playback speed

The speed control next to the rewind buttons offers 0.5x, 1x, 1.25x, 1.5x and
2x. It is enabled only while reviewing in CMAF mode, where the fetched fragments
play through a MediaSource and the element's `playbackRate` applies; live
playback has to keep pace with the publisher and the WebCodecs path paces
frames itself. Returning to live resets the rate to 1x. Audio, once review
carries it, is time-stretched by the browser (`preservesPitch` is on by default).

## Player controls

The seek bar, the rewind buttons and the quality menu sit on the video itself
rather than in their own cards. The gear opens the video and audio track
selection along with the jitter buffer switch, and closes on a second click, on
Escape, or on a click outside it.

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

The position label shows seconds behind live and follows review playback. Seeking
starts at the preceding closed keyframe group and uses the same bounded FETCH
replay as the rewind buttons.

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

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

live-ingest は各 object を `[meta_len (u32 BE)][meta JSON][coded data]` で送り、ブラウザ publisher は
coded data のみを送って metadata を MoQT の LOC 拡張ヘッダに載せます。`utils/media/ingestChunk.ts` が
両方を受け付け、前者は JSON を外して capture timestamp を LOC ヘッダ相当に変換します。

## E2E

relay・live-ingest・配信元・dev server が動いている状態で実行します。

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=live \
npm --prefix examples/browser run e2e:live-viewer
```

## Seek bar

The slider below the video shows the time range observed on the selected video
track. Drag it or use the arrow keys to choose a position; Home selects the
oldest observed group and End returns to live. The position label shows seconds
behind live and follows review playback. Seeking starts at the preceding closed
keyframe group and uses the same bounded FETCH replay as the rewind buttons.

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

The label under the slider reads `position / broadcast`, both measured from the
start of the broadcast. The numbers come from the MSF media timeline track
(draft-ietf-moq-msf-00 section 7), which the bridge publishes as a JSON array of
`[presentation time, [group id, object id], encode wallclock]` records covering
the groups the relay still caches. The viewer finds it in the catalog by its
`mediatimeline` packaging and subscribes to it alongside the media tracks.

A capture timestamp resolves to a presentation time by offsetting from the
newest record at or before it, so the reading survives a video quality change
even though renditions number their groups independently. The label shows
`--:-- / --:--` until the first timeline object arrives.

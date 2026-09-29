# live-viewer

`bridges/live-ingest` が RTMP / SRT から MoQT へ流した配信や、このページから配信した MP4 を視聴します。catalog に載っている映像
track を切り替えられるので、`--transcode` で生成した下位画質（`video_480p` / `video_360p`）の確認にも
使えます。

## 配信の開始方法

次の4経路から選びます。コマンドを使う経路では、各コマンドを別々のターミナルで実行します。
OBS / ffmpeg では映像に H.264、音声に AAC を使用します。以下の ffmpeg コマンドは Big Buck Bunny を繰り返し配信します。

<details>
<summary>MP4 ファイル（ブラウザ）→ クラウド Relay</summary>

1. **MP4 を配信する**

   MP4 Publish で映像が H.264、音声が AAC または MP3 の MP4 を選び、Publish を押します。Relay URL と namespace は Stream のものを使い、
   ブラウザが MP4 を demux して LOC で配信します。Loop を外すとファイルの終わりで配信を終えます。

2. **Live Viewer で視聴する**

   同じ Relay URL と namespace のまま Watch を押します。別のブラウザから同じ namespace を視聴することもできます。

</details>

<details>
<summary>OBS / ffmpeg → クラウド SRT / RTMP Ingestion (live-ingest) → クラウド Relay</summary>

1. **配信元を選ぶ（ffmpeg または OBS）**

   次のどちらか一方を選んで配信を開始します。

   **A. ffmpeg を使う**

   ```shell
   make ffmpeg-srt-bbb-remote
   ```

   **B. OBS を使う**

   SRT のサーバー URL に以下を設定して配信を開始します。

   ```text
   srt://relay-1.moqt.research.skyway.io:9000?mode=caller&streamid=anon/live/test
   ```

   RTMP を使う場合は、サーバーに `rtmp://relay-1.moqt.research.skyway.io:1935/anon/live/test`、ストリームキーに `stream` を設定します。

2. **Live Viewer で視聴する**

   視聴先の Relay URL： `https://relay-1.moqt.research.skyway.io:443`

   namespace に `anon/live/test` を指定し、Watch を押します。

</details>

<details>
<summary>OBS / ffmpeg → ローカル Ingestion (GStreamer) → クラウド Relay</summary>

1. **ローカル Ingestion を起動する**

   ```shell
   make gst-srt-publish GST_MOQT_URL=https://relay-1.moqt.research.skyway.io:443
   ```

2. **配信元を選ぶ（ffmpeg または OBS）**

   次のどちらか一方を選んで配信を開始します。

   **A. ffmpeg を使う**

   ```shell
   make ffmpeg-srt-bbb-local
   ```

   テストパターンを使う場合は、代わりに `make ffmpeg-srt` を実行します。

   **B. OBS を使う**

   SRT のサーバー URL に以下を設定して配信を開始します。

   ```text
   srt://localhost:9000?mode=caller&streamid=anon/live/test
   ```

3. **Live Viewer で視聴する**

   視聴先の Relay URL： `https://relay-1.moqt.research.skyway.io:443`

   namespace に `anon/live/test` を指定し、Watch を押します。

</details>

<details>
<summary>OBS / ffmpeg → ローカル Ingestion (GStreamer) → ローカル Relay</summary>

1. **ローカル Relay を起動する**

   ```shell
   make relay
   ```

2. **ローカル Ingestion を起動する**

   ```shell
   make gst-srt-publish GST_MOQT_URL=https://127.0.0.1:4433
   ```

3. **配信元を選ぶ（ffmpeg または OBS）**

   次のどちらか一方を選んで配信を開始します。

   **A. ffmpeg を使う**

   ```shell
   make ffmpeg-srt-bbb-local
   ```

   テストパターンを使う場合は、代わりに `make ffmpeg-srt` を実行します。

   **B. OBS を使う**

   SRT のサーバー URL に以下を設定して配信を開始します。

   ```text
   srt://localhost:9000?mode=caller&streamid=anon/live/test
   ```

4. **Live Viewer で視聴する**

   別々のターミナルで以下を実行し、Live Viewer を開きます。

   ```shell
   make browser

   make chrome
   ```

   視聴先の Relay URL： `https://127.0.0.1:4433`

   namespace に `anon/live/test` を指定し、Watch を押します。

</details>

`examples/live-viewer/index.html` を開き、配信経路に合った Relay URL と
namespace `anon/live/test` を選んで Watch を押します。GStreamer 用コマンドは
`GST_NAMESPACE`（既定値：`anon/live/test`）で配信し、live-ingest は SRT の stream ID
または RTMP の app パスを使用します。`?moqtUrl=...&trackNamespace=...` でも指定できます。

## 巻き戻し

映像にカーソルを合わせるとシークバーと、中央に `↺5` `↺1` `1↻` `5↻` が出ます。キーボードでは ← / → が 1 秒、
↓ / ↑ が 5 秒です。どれも relay のキャッシュに残っている group を FETCH で取り出し、映像は review canvas
に、音声は review 用の AudioContext に、capture timestamp のとおりのペースで揃えて再生します。`LIVE` で
ライブ表示へ戻ります。

- 位置は、ライブ再生中に観測した capture timestamp から求めます。bridge は group id を
  壁時計で採番し、group はエンコーダの keyframe ごとに切り替わるため、group id の差は秒数になりません。
- 目標位置を含む閉じた keyframe group から FETCH し、目標より前のフレームはペースをかけずにデコード
  だけして、目標以降のフレームから描画します。MSE では同じ分だけ `currentTime` を進めて再生を始めます。
- 取得範囲は publisher が書き込みを終えた group までに制限します。開いている group に伸ばすと
  relay のキャッシュを外れて上流へ転送され、publisher 側キャッシュ（60 秒）から返されます。
- relay のキャッシュ保持は既定 60 秒（`RELAY_CACHE_TTL_SECS`）、publisher 側も 60 秒です。
  それより前へは戻れません。
- 巻き戻し中は video と audio の SUBSCRIBE を SUBSCRIBE_UPDATE で Forward 0 にし、ライブの object を
  受け取りません。その間に publisher が開いた group は 500 ms ごとの TRACK_STATUS の Largest Location で
  知り、次に FETCH する範囲とシークバーの右端を進めます。group の位置は media timeline が記録する encode
  時刻から取ります。media timeline が対象にしない rendition では、最後に受け取ったライブ object の遅れから
  推定するので、main thread が忙しいと先へずれます。`LIVE` で Forward 1 に戻すと relay は次に開いた group から配信を再開するので、
  ライブの絵が動き出すまで最大で 1 GOP ほどかかります。TRACK_STATUS に応えない relay では、巻き戻し中も
  Forward 1 に戻して従来どおりライブを受け取ります。

## MP4 の配信

MP4 Publish は、選んだファイルをブラウザの中で demux し、live-ingest と同じ形の catalog と LOC track
（`video` / `audio`）として relay へ配信します。demux は `shared/mediapack` の progressive MP4 index
（`mp4::Mp4Index`）を `bindings/wasm` 経由で使い、`moov` だけを wasm に渡してサンプルは `File.slice` で
読むので、ファイル全体をメモリに載せません。視聴側は live-ingest の配信と同じ経路で再生し、巻き戻しも
同じように動きます。CMAF の sibling track は配信しないため、Packaging は LOC のみです。media timeline は
live-ingest と同じく配信するので、シークバーの経過時間も出ます。

- 映像は H.264 のみです。mediapack が AVCC サンプルを Annex B に直し、keyframe の前に `avcC` の SPS / PPS を
  付けるので、live-ingest と同じく catalog に `initData` はありません。
- 音声は AAC と MP3 に対応し、どちらも MP4 のフレームをそのまま送ります（LOC の payload は WebCodecs の
  codec registry にあるコーデックの生ビットストリーム）。AAC は AudioSpecificConfig を catalog の
  `initData` に載せ、MP3 は catalog の `codec: "mp3"` だけで受信側の `AudioDecoder` が設定されます。
  ほかのコーデックの音声は飛ばして映像だけを配信します。
- 視聴者の SUBSCRIBE は待ちません。配信を始めると `video` / `audio` / `timeline` / `catalog` を PUBLISH し、catalog は
  それが載せる track をすべて PUBLISH してから送るので、relay は各 track を最初の object からキャッシュします。
  catalog は relay のキャッシュから落ちないよう 30 秒ごとに送り直します。publisher に届いた SUBSCRIBE は
  NOT_SUPPORTED で断ります。
- group は video の keyframe ごとに切り替え、audio の object は直前の keyframe の group に入れます。
  group id は catalog も含めて開始時刻（unix マイクロ秒）から採番するので、配信し直しても同じ location を
  再利用しません（relay は publisher が替わっても track のキャッシュを保持し、既知の location を malformed
  track として扱います）。
- media timeline（`timeline` track）は video の keyframe ごとに、それまでの record（presentation time、
  `[group id, 0]`、encode wallclock）を 1 つの object として新しい group に載せます。record は relay のキャッシュ
  保持（60 秒）より古いものを捨て、presentation time は配信を始めた時点からの経過です。
- 各サンプルは B フレームを含むライブエンコーダと同じく decode 順に、decode time にファイルの reorder delay
  （presentation time が decode time より進む最大量。B フレームがなければ 0）を足した壁時計で送り、presentation
  time の壁時計を capture timestamp として LOC 拡張ヘッダに載せます。Loop のときは、次の周回をファイルの長さぶん
  後ろにずらして続けます。
- 配信中は MP4 Publish の中に、左に送信中の映像の小さなプレビュー、右に Publish Streams を出します。Publish Streams は
  relay へ送った group(subgroup stream)を、Playback の映像のすぐ下にある Subscribe Streams と同じ横棒で track ごとに
  示し、Window と GOPs は Subscribe Streams の設定を共有します。見出しの横には送信中 / 送信済みの stream 数と、窓内に
  開いた stream の送信ビットレートを出します。
- プレビューは、送るサンプルをそのまま WebCodecs でデコードし、各フレームを capture timestamp に reorder delay を
  足した時刻（そのフレームまでがすべて送られた時刻）に描くので、ネットワークとバッファの遅延がない受信側の絵に
  なります。viewer の LOC フレームも同じ capture timestamp を持つため、表示した時刻との差を `viewer delay` として
  プレビューの下に、`delay` として Playback の統計に出します。別のブラウザで視聴するときは、両者の壁時計のずれが
  そのまま差に乗ります。
- 配信は視聴とは別の MoQT セッションで行うので、同じページで Watch / Stop を押しても配信は続きます。
  ブラウザは FETCH に応えないため、巻き戻しは relay のキャッシュにある閉じた group の範囲になります。

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

MP4 の配信は relay と dev server だけで実行できます。ffmpeg で生成したテスト用 MP4（H.264 baseline に AAC
または MP3）をブラウザから配信し、同じページで視聴します。

```shell
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
npm --prefix examples/browser run e2e:live-viewer-mp4
```

## Delivery check

`e2e:live-viewer-delivery` watches a stream for `DELIVERY_SECONDS` (default 30) and records every object the viewer hands to its decoder workers, then
reads the bridge's delivery log and reports, per track, how many samples were
published within the span the viewer watched, how many of them it received,
which are missing, and how many samples entered the bridge but were not
published (dropped before the first keyframe or after a transport-stream
loss). Run the bridge with the delivery log on and point the test at it:

```shell
RUST_LOG=info,media_publisher::delivery=debug make live-ingest 2>&1 | tee /tmp/live-ingest.log
DELIVERY_BRIDGE_LOG=/tmp/live-ingest.log \
DELIVERY_SECONDS=60 \
MEDIA_E2E_BASE_URL=http://127.0.0.1:5173 \
MEDIA_E2E_MOQT_URL=https://127.0.0.1:4433 \
LIVE_VIEWER_E2E_NAMESPACE=anon/live/test \
npm --prefix examples/browser run e2e:live-viewer-delivery
```

Samples are matched by their LOC capture timestamp, so the check covers the
LOC tracks; it fails when any published sample is missing.

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
stays on screen until then. The live MediaSource stays open hidden behind a
review but receives nothing while the live subscriptions do not forward, so
after `LIVE` the fragments of the next group land in a buffered range of their
own and playback jumps there once it holds a second of video. The live audio
still buffered when a review starts is silenced and heard again on `LIVE`.

## Review audio

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

## Catalog

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

## Audio / video synchronisation

In LOC mode the two decoder workers hand every sample over as soon as it is
decoded, and one playout clock decides when each is presented. The clock maps
the LOC capture timestamps, which the bridge stamps on the same wall clock for
every track, onto the local clock behind a jitter buffer. With `Buffer`
set to `Adaptive` (the default) the buffer tracks the delay from capture to
arrival of the audio chunks of the last 10 s and covers the spread between the
fastest and the slowest of them, never less than `Min buffer (ms)` (200 ms by
default), plus `Extra delay (ms)` as a margin for swings wider than the last
10 s have shown. With `Fixed` it is `Buffer (ms)` whatever the spread. Either
way it also covers the audio device's output latency. A subscription opens
with a burst of what the relay had cached of the current groups, so the
samples of the first 400 ms are held and the clock is anchored on the newest
of them; older ones are dropped rather than played late. That burst says
nothing about the spread, so an adaptive buffer opens on the longest wait
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
settled (10 s for an adaptive one), `Catch up` decides how a buffer above its
target shrinks. `Skip`, the default, drops each audio chunk that fits in the
excess and fades the next one in from the head of the first dropped chunk, at
the offset within 10 ms where the two waveforms look most alike, so the sound
jumps without a click. `Speed up` shortens the audio by 10 % while the buffer
is more than 20 ms over, without changing its pitch: where the waveform
repeats, one period of 2.5–10 ms is overlapped onto the next with a 5 ms
crossfade (the time-scale modification WebRTC's NetEq calls accelerate), and
it skips like `Skip` once the buffer is more than 400 ms over. `Off` never
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

The Relay URL defaults to Cloud relay-1. Presets include the cloud load balancer,
relay-1 through relay-3, and local relay-a / relay-b, using the same URLs as Meeting.
An explicit `?moqtUrl=...` overrides the default.
